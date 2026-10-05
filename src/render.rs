//! Draws a viewport of a [`Map`] into a pixmap.
//!
//! Per viewport: query the zoom-bucketed index, walk the (pre-sorted) features
//! in runs that share a draw group and layer, batch consecutive features with
//! identical style into one path, and rasterize with anti-aliasing. Geometry is
//! projected, decimated to sub-pixel tolerance and clipped to the viewport
//! (plus a margin for stroke width) before it reaches the rasterizer, so huge
//! polygons cost little in tiles that only see a corner of them.

use crate::classify::{Group, Kind};
use crate::geo::{clip_polyline, clip_ring, meters_per_unit, Piece, Point, Rect, TILE_SIZE};
use crate::map::{Feature, Map};
use crate::style::{Ctx, LineStyle, StrokeSpec, Theme};
use tiny_skia::{
    BlendMode, Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, StrokeDash,
    Transform,
};

/// Simplification tolerance in pixels: vertices closer than this to the
/// previous kept vertex are dropped.
const TOLERANCE: f64 = 0.3;
/// Buildings cast shadows from this zoom on, fading in over one level.
const SHADOW_ZOOM: f32 = 14.5;
/// Buildings are extruded from this zoom on (in [`Buildings::Extruded`] mode).
const EXTRUDE_ZOOM: f32 = 15.0;
/// Screen height of a building relative to its true scale: an oblique view
/// tilted roughly 35° from straight down.
const EXTRUDE_SCALE: f64 = 0.6;
/// Cap on extrusion height in pixels (at scale 1), so supertall towers stay
/// within a bounded query margin.
const MAX_EXTRUDE: f64 = 512.0;

/// How buildings are drawn at high zoom.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Buildings {
    /// Flat footprints with cast shadows.
    Flat,
    /// 2.5D: walls and roofs extruded by building height.
    #[default]
    Extruded,
}

/// A rectangle of the world at a given zoom, in output pixels.
#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    /// Top-left corner in world pixels at this zoom and scale.
    pub x: f64,
    pub y: f64,
    pub width: u32,
    pub height: u32,
    /// Style zoom (fractional zooms are fine).
    pub zoom: f64,
    /// Pixel ratio: 2 renders the same zoom at double resolution.
    pub scale: f32,
}

impl Viewport {
    /// The XYZ tile `(z, x, y)`.
    pub fn tile(z: u8, x: u32, y: u32, scale: f32) -> Self {
        let size = (TILE_SIZE as f32 * scale).round() as u32;
        Viewport {
            x: x as f64 * size as f64,
            y: y as f64 * size as f64,
            width: size,
            height: size,
            zoom: z as f64,
            scale,
        }
    }

    /// World width in output pixels.
    pub fn world_px(&self) -> f64 {
        TILE_SIZE * self.scale as f64 * self.zoom.exp2()
    }
}

/// Reusable per-thread buffers.
#[derive(Default)]
struct Scratch {
    ids: Vec<u32>,
    pts: Vec<Point>,
    tmp: Vec<Point>,
}

/// Transformed, clipped line geometry for one style batch.
#[derive(Default)]
struct LineGeom {
    points: Vec<Point>,
    pieces: Vec<Piece>,
}

pub struct Renderer<'a> {
    map: &'a Map,
    theme: &'a Theme,
    buildings: Buildings,
}

/// Local → pixel transform and the per-viewport style context.
struct Frame {
    k: f64,
    bx: f64,
    by: f64,
    width: f64,
    height: f64,
    ctx: Ctx,
    vis_zoom: f32,
}

impl Frame {
    fn rect(&self, margin: f64) -> Rect {
        Rect::new(-margin, -margin, self.width + margin, self.height + margin)
    }

    /// Feature bounds in pixels.
    fn bbox(&self, f: &Feature) -> Rect {
        let b = f.bbox;
        Rect::new(
            b[0] as f64 * self.k + self.bx,
            b[1] as f64 * self.k + self.by,
            b[2] as f64 * self.k + self.bx,
            b[3] as f64 * self.k + self.by,
        )
    }

    /// Projects a ring into `out`, dropping vertices within [`TOLERANCE`] of
    /// the previous one (the last vertex is always kept).
    fn project(&self, ring: &[[f32; 2]], out: &mut Vec<Point>) {
        out.clear();
        let n = ring.len();
        for (i, p) in ring.iter().enumerate() {
            let q = [
                p[0] as f64 * self.k + self.bx,
                p[1] as f64 * self.k + self.by,
            ];
            if let Some(last) = out.last() {
                let close =
                    (q[0] - last[0]).abs() < TOLERANCE && (q[1] - last[1]).abs() < TOLERANCE;
                if close && i + 1 < n {
                    continue;
                }
            }
            out.push(q);
        }
    }
}

fn paint(color: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color);
    p.anti_alias = true;
    p
}

fn stroke(spec: &StrokeSpec, dash_offset: f64) -> Stroke {
    Stroke {
        width: spec.width,
        line_cap: if spec.round {
            LineCap::Round
        } else {
            LineCap::Butt
        },
        line_join: if spec.round {
            LineJoin::Round
        } else {
            LineJoin::Miter
        },
        dash: spec.dash.and_then(|[on, off]| {
            let phase = dash_offset.rem_euclid((on + off) as f64) as f32;
            StrokeDash::new(vec![on, off], phase)
        }),
        ..Stroke::default()
    }
}

fn push_polyline(pb: &mut PathBuilder, pts: &[Point]) {
    pb.move_to(pts[0][0] as f32, pts[0][1] as f32);
    for p in &pts[1..] {
        pb.line_to(p[0] as f32, p[1] as f32);
    }
}

fn push_ring(pb: &mut PathBuilder, pts: &[Point]) {
    if pts.len() >= 3 {
        push_polyline(pb, pts);
        pb.close();
    }
}

impl<'a> Renderer<'a> {
    pub fn new(map: &'a Map, theme: &'a Theme, buildings: Buildings) -> Self {
        Renderer {
            map,
            theme,
            buildings,
        }
    }

    pub fn render(&self, vp: &Viewport) -> Pixmap {
        let mut pixmap = Pixmap::new(vp.width, vp.height).expect("viewport has a non-zero size");
        self.render_into(vp, &mut pixmap);
        pixmap
    }

    /// Whether the viewport overlaps the map data at all.
    pub fn intersects(&self, vp: &Viewport) -> bool {
        let k = vp.world_px();
        let view = Rect::new(
            vp.x / k,
            vp.y / k,
            (vp.x + vp.width as f64) / k,
            (vp.y + vp.height as f64) / k,
        );
        view.intersects(&self.map.bounds)
    }

    pub fn render_into(&self, vp: &Viewport, pixmap: &mut Pixmap) {
        let map = self.map;
        let k = vp.world_px();
        let center_y = (vp.y + vp.height as f64 / 2.0) / k;
        let frame = Frame {
            k,
            bx: map.origin[0] * k - vp.x,
            by: map.origin[1] * k - vp.y,
            width: vp.width as f64,
            height: vp.height as f64,
            ctx: Ctx {
                zoom: vp.zoom as f32,
                scale: vp.scale,
                ppm: (k / meters_per_unit(center_y.clamp(0.0, 1.0))) as f32,
            },
            vis_zoom: (vp.zoom + (vp.scale as f64).log2()) as f32,
        };

        pixmap.fill(match map.background {
            crate::assemble::Background::Land => self.theme.land_color(),
            crate::assemble::Background::Water => self.theme.water_color(),
        });

        let mut s = Scratch::default();
        let extrude = self.buildings == Buildings::Extruded && frame.ctx.zoom >= EXTRUDE_ZOOM;
        // Query generously: wide strokes and shadows reach beyond feature
        // bounds, and extruded buildings south of the view rise into it.
        let margin = 160.0 * vp.scale as f64;
        let below = if extrude {
            margin + MAX_EXTRUDE * vp.scale as f64
        } else {
            margin
        };
        let to_local = |px: f64, origin: f64| ((px / k) - origin) as f32;
        let area = [
            to_local(vp.x - margin, map.origin[0]),
            to_local(vp.y - margin, map.origin[1]),
            to_local(vp.x + vp.width as f64 + margin, map.origin[0]),
            to_local(vp.y + vp.height as f64 + below, map.origin[1]),
        ];
        map.query(area, frame.vis_zoom, &mut s.ids);
        let ids = std::mem::take(&mut s.ids);

        // Extruded buildings are drawn after ground-level streets and trees,
        // which they occlude, but before bridges and overlays.
        let mut deferred: Option<&[u32]> = None;
        let mut i = 0;
        while i < ids.len() {
            let first = &map.features[ids[i] as usize];
            let group = first.group();
            let layer = first.layer;
            let transport = group.is_transport();
            let mut j = i + 1;
            while j < ids.len() {
                let f = &map.features[ids[j] as usize];
                if f.group() != group || (transport && f.layer != layer) {
                    break;
                }
                j += 1;
            }
            let run = &ids[i..j];
            if group > Group::Trees {
                if let Some(buildings) = deferred.take() {
                    self.draw_extruded(pixmap, &frame, buildings, &mut s);
                }
            }
            match group {
                Group::Land | Group::Areas | Group::AreaOverlays => {
                    self.draw_areas(pixmap, &frame, run, &mut s)
                }
                Group::Buildings if extrude => deferred = Some(run),
                Group::Buildings => {
                    if frame.ctx.zoom >= SHADOW_ZOOM {
                        self.draw_shadows(pixmap, &frame, run, &mut s);
                    }
                    self.draw_areas(pixmap, &frame, run, &mut s);
                }
                Group::Trees => self.draw_trees(pixmap, &frame, run),
                _ => self.draw_lines(pixmap, &frame, run, &mut s),
            }
            i = j;
        }
        if let Some(buildings) = deferred {
            self.draw_extruded(pixmap, &frame, buildings, &mut s);
        }

        self.mask_outside(pixmap, &frame);
    }

    /// Splits a run into batches of consecutive features drawn identically.
    fn batches<'r>(&'r self, run: &'r [u32]) -> impl Iterator<Item = &'r [u32]> + 'r {
        let features = &self.map.features;
        run.chunk_by(move |&a, &b| {
            let (a, b) = (&features[a as usize], &features[b as usize]);
            a.kind == b.kind
                && a.flags == b.flags
                && (a.kind != Kind::Boundary || a.height == b.height)
        })
    }

    fn draw_areas(&self, pixmap: &mut Pixmap, fr: &Frame, run: &[u32], s: &mut Scratch) {
        for batch in self.batches(run) {
            let f0 = &self.map.features[batch[0] as usize];
            let Some(style) = self.theme.area(f0.kind, &fr.ctx) else {
                continue;
            };
            let margin = style.outline.as_ref().map_or(0.0, |o| o.width as f64) + 2.0;
            let clip = fr.rect(margin);
            let mut pb = PathBuilder::new();
            for &id in batch {
                let f = &self.map.features[id as usize];
                let inside = clip.contains([fr.bbox(f).min_x, fr.bbox(f).min_y])
                    && clip.contains([fr.bbox(f).max_x, fr.bbox(f).max_y]);
                for ring in self.map.rings(f) {
                    fr.project(ring, &mut s.pts);
                    if !inside {
                        clip_ring(&mut s.pts, &clip, &mut s.tmp);
                    }
                    push_ring(&mut pb, &s.pts);
                }
            }
            let Some(path) = pb.finish() else { continue };
            pixmap.fill_path(
                &path,
                &paint(style.fill),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
            if let Some(outline) = &style.outline {
                pixmap.stroke_path(
                    &path,
                    &paint(outline.color),
                    &stroke(outline, 0.0),
                    Transform::identity(),
                    None,
                );
            }
        }
    }

    /// Pseudo-3D shadows: each building's footprint swept along the light
    /// direction `d` by its height.
    ///
    /// Tracing any point of the sweep back against `d`, it either lies in the
    /// footprint (which the building fill covers anyway) or first enters it
    /// through an edge whose outward normal faces `d`. So the shadow is just
    /// the union of the quads those edges sweep. Rings are normalized so the
    /// footprint is always on the left of travel; such edges are exactly those
    /// with `cross(b - a, d) < 0`, and every emitted quad then winds the same
    /// way, so one non-zero fill unions them without darkening overlaps.
    fn draw_shadows(&self, pixmap: &mut Pixmap, fr: &Frame, run: &[u32], s: &mut Scratch) {
        let fade = (fr.ctx.zoom - SHADOW_ZOOM).clamp(0.0, 1.0);
        let mut color = self.theme.shadow();
        color.set_alpha(color.alpha() * fade);
        // Short, capped shadows: enough to give depth without hiding streets.
        let max_len = 40.0 * fr.ctx.scale as f64;
        let clip = fr.rect(max_len + 2.0);
        let mut pb = PathBuilder::new();
        for &id in run {
            let f = &self.map.features[id as usize];
            let height = if f.height > 0.0 { f.height } else { 8.0 };
            let len = (height as f64 * fr.ctx.ppm as f64 * 0.22).min(max_len);
            if len < 0.5 {
                continue;
            }
            let d = [len * 0.6, len * 0.8];
            let bbox = fr.bbox(f);
            let inside =
                clip.contains([bbox.min_x, bbox.min_y]) && clip.contains([bbox.max_x, bbox.max_y]);
            for ring in self.map.rings(f) {
                fr.project(ring, &mut s.pts);
                if !inside {
                    clip_ring(&mut s.pts, &clip, &mut s.tmp);
                }
                let n = s.pts.len();
                if n < 3 {
                    continue;
                }
                for e in 0..n {
                    let a = s.pts[e];
                    let b = s.pts[(e + 1) % n];
                    let cross = (b[0] - a[0]) * d[1] - (b[1] - a[1]) * d[0];
                    if cross < 0.0 {
                        let quad = [a, b, [b[0] + d[0], b[1] + d[1]], [a[0] + d[0], a[1] + d[1]]];
                        push_ring(&mut pb, &quad);
                    }
                }
            }
        }
        if let Some(path) = pb.finish() {
            pixmap.fill_path(
                &path,
                &paint(color),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    /// 2.5D buildings in an oblique view from the south: each roof is the
    /// footprint lifted straight up the screen by the building's height, and
    /// the walls are the quads swept by footprint edges that face the viewer
    /// (roof plus those walls cover the whole footprint, so it needs no fill
    /// of its own). Walls are shaded by facing against light from the
    /// west-southwest. Buildings are painted back to front, ordered by their
    /// southern edge, so nearer buildings occlude farther ones; the order is
    /// global, so it is identical in neighbouring tiles.
    fn draw_extruded(&self, pixmap: &mut Pixmap, fr: &Frame, run: &[u32], s: &mut Scratch) {
        const SHADES: usize = 4;
        let features = &self.map.features;
        let mut order = run.to_vec();
        order.sort_by(|&a, &b| {
            let (fa, fb) = (&features[a as usize], &features[b as usize]);
            fa.bbox[3].total_cmp(&fb.bbox[3]).then(a.cmp(&b))
        });

        let scale = fr.ctx.scale as f64;
        let cap = MAX_EXTRUDE * scale;
        let clip = fr.rect(cap + 2.0);
        let light = {
            let (x, y) = (-1.0_f64, 0.5_f64);
            let len = (x * x + y * y).sqrt();
            [x / len, y / len]
        };
        let roof = paint(self.theme.roof());
        let walls: Vec<Paint> = (0..SHADES)
            .map(|i| paint(self.theme.facade(i as f32 / (SHADES - 1) as f32)))
            .collect();
        let outline = (fr.ctx.zoom >= 16.0).then(|| {
            let spec = StrokeSpec {
                color: self.theme.building_outline(),
                width: 0.6 * fr.ctx.scale,
                dash: None,
                round: true,
            };
            (paint(spec.color), stroke(&spec, 0.0))
        });

        for id in order {
            let f = &features[id as usize];
            let height = if f.height > 0.0 { f.height } else { 6.0 };
            let lift = (height as f64 * fr.ctx.ppm as f64 * EXTRUDE_SCALE).min(cap);
            let bbox = fr.bbox(f);
            let inside =
                clip.contains([bbox.min_x, bbox.min_y]) && clip.contains([bbox.max_x, bbox.max_y]);
            let mut wall_paths: [Option<PathBuilder>; SHADES] = Default::default();
            let mut roof_path = PathBuilder::new();
            for ring in self.map.rings(f) {
                fr.project(ring, &mut s.pts);
                if !inside {
                    clip_ring(&mut s.pts, &clip, &mut s.tmp);
                }
                let n = s.pts.len();
                if n < 3 {
                    continue;
                }
                for e in 0..n {
                    let (a, b) = (s.pts[e], s.pts[(e + 1) % n]);
                    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                    // Rings keep the footprint on their left, so the outward
                    // normal is (dy, -dx): the edge faces the viewer (down the
                    // screen) when dx < 0.
                    if dx >= 0.0 || lift < 0.5 {
                        continue;
                    }
                    let len = (dx * dx + dy * dy).sqrt();
                    let lit = ((dy * light[0] - dx * light[1]) / len).max(0.0);
                    let shade = ((lit * SHADES as f64) as usize).min(SHADES - 1);
                    let quad = [a, b, [b[0], b[1] - lift], [a[0], a[1] - lift]];
                    push_ring(
                        wall_paths[shade].get_or_insert_with(PathBuilder::new),
                        &quad,
                    );
                }
                s.tmp.clear();
                s.tmp.extend(s.pts.iter().map(|p| [p[0], p[1] - lift]));
                push_ring(&mut roof_path, &s.tmp);
            }
            for (pb, p) in wall_paths.into_iter().zip(&walls) {
                if let Some(path) = pb.and_then(PathBuilder::finish) {
                    pixmap.fill_path(&path, p, FillRule::Winding, Transform::identity(), None);
                }
            }
            if let Some(path) = roof_path.finish() {
                pixmap.fill_path(&path, &roof, FillRule::Winding, Transform::identity(), None);
                if let Some((p, st)) = &outline {
                    pixmap.stroke_path(&path, p, st, Transform::identity(), None);
                }
            }
        }
    }

    fn draw_trees(&self, pixmap: &mut Pixmap, fr: &Frame, run: &[u32]) {
        let (color, r) = self.theme.tree(&fr.ctx);
        let clip = fr.rect(r as f64);
        let mut pb = PathBuilder::new();
        for &id in run {
            let f = &self.map.features[id as usize];
            let p = self.map.ring(f.ring_start)[0];
            let q = [p[0] as f64 * fr.k + fr.bx, p[1] as f64 * fr.k + fr.by];
            if clip.contains(q) {
                pb.push_circle(q[0] as f32, q[1] as f32, r);
            }
        }
        if let Some(path) = pb.finish() {
            pixmap.fill_path(
                &path,
                &paint(color),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    /// Lines draw in two passes over the run: every casing, then every line,
    /// so that casings never cut across crossing roads of the same layer.
    fn draw_lines(&self, pixmap: &mut Pixmap, fr: &Frame, run: &[u32], s: &mut Scratch) {
        let mut batches: Vec<(LineStyle, LineGeom)> = Vec::new();
        for batch in self.batches(run) {
            let f0 = &self.map.features[batch[0] as usize];
            let style = self.theme.line(f0.kind, f0.flags, f0.height, &fr.ctx);
            let widest = [&style.casing, &style.line]
                .iter()
                .filter_map(|s| s.as_ref().map(|s| s.width))
                .fold(0.0_f32, f32::max);
            if widest <= 0.0 {
                continue;
            }
            let clip = fr.rect(widest as f64 / 2.0 + 2.0);
            let mut geom = LineGeom::default();
            for &id in batch {
                let f = &self.map.features[id as usize];
                if !fr.bbox(f).intersects(&clip) {
                    continue;
                }
                for ring in self.map.rings(f) {
                    fr.project(ring, &mut s.pts);
                    if s.pts.len() >= 2 {
                        clip_polyline(&s.pts, &clip, &mut geom.points, &mut geom.pieces);
                    }
                }
            }
            if !geom.pieces.is_empty() {
                batches.push((style, geom));
            }
        }
        for pass in 0..2 {
            for (style, geom) in &batches {
                let spec = if pass == 0 {
                    &style.casing
                } else {
                    &style.line
                };
                if let Some(spec) = spec {
                    stroke_pieces(pixmap, geom, spec);
                }
            }
        }
    }

    /// Paints everything outside the data bounds with the theme's "outside"
    /// color, or clears it to transparent.
    fn mask_outside(&self, pixmap: &mut Pixmap, fr: &Frame) {
        let b = &self.map.bounds;
        let (ox, oy) = (self.map.origin[0], self.map.origin[1]);
        let x0 = ((b.min_x - ox) * fr.k + fr.bx) as f32;
        let y0 = ((b.min_y - oy) * fr.k + fr.by) as f32;
        let x1 = ((b.max_x - ox) * fr.k + fr.bx) as f32;
        let y1 = ((b.max_y - oy) * fr.k + fr.by) as f32;
        let (w, h) = (fr.width as f32, fr.height as f32);
        let mut p = match self.theme.outside_color() {
            Some(c) => paint(c),
            None => paint(Color::TRANSPARENT),
        };
        p.blend_mode = BlendMode::Source;
        p.anti_alias = false;
        let rects = [
            (0.0, 0.0, w, y0.max(0.0)),
            (0.0, y1.min(h), w, h),
            (0.0, y0.max(0.0), x0.max(0.0), y1.min(h)),
            (x1.min(w), y0.max(0.0), w, y1.min(h)),
        ];
        for (l, t, r, btm) in rects {
            // `from_ltrb` accepts empty rects, which still paint a pixel row.
            if r - l < 0.5 || btm - t < 0.5 {
                continue;
            }
            if let Some(rect) = tiny_skia::Rect::from_ltrb(l, t, r, btm) {
                pixmap.fill_rect(rect, &p, Transform::identity(), None);
            }
        }
    }
}

/// Strokes clipped line pieces. Solid strokes go out as one path; dashed ones
/// stroke per piece, offset by the distance travelled so far along the
/// original line, so dash patterns stay continuous across tile edges.
fn stroke_pieces(pixmap: &mut Pixmap, geom: &LineGeom, spec: &StrokeSpec) {
    let p = paint(spec.color);
    if spec.dash.is_none() {
        let mut pb = PathBuilder::new();
        for piece in &geom.pieces {
            push_polyline(&mut pb, &geom.points[piece.start..piece.start + piece.len]);
        }
        if let Some(path) = pb.finish() {
            pixmap.stroke_path(&path, &p, &stroke(spec, 0.0), Transform::identity(), None);
        }
        return;
    }
    for piece in &geom.pieces {
        let mut pb = PathBuilder::new();
        push_polyline(&mut pb, &geom.points[piece.start..piece.start + piece.len]);
        if let Some(path) = pb.finish() {
            let st = stroke(spec, piece.distance);
            pixmap.stroke_path(&path, &p, &st, Transform::identity(), None);
        }
    }
}
