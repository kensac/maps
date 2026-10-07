//! Solids extruded from their footprints: buildings, building parts, tanks,
//! canopies, bleachers and tombs. In flat views they are footprints with
//! cast shadows instead.

use super::camera::Frame;
use super::mesh::Mesh;
use super::paint::{paint, push_ring, stroke};
use super::{Renderer, Scratch, SHADOW_ZOOM};
use crate::classify::{Detail, Kind, RoofShape};
use crate::geo::{clip_ring, Point};
use crate::map::Feature;
use crate::style::StrokeSpec;
use tiny_skia::{Color, FillRule, LineCap, PathBuilder, Pixmap, Stroke, Transform};

/// Wall shading levels per solid.
const SHADES: usize = 4;
/// Thickness of a canopy roof slab, in meters.
const CANOPY_SLAB: f32 = 0.4;

impl Renderer<'_> {
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
    pub(super) fn draw_shadows(
        &self,
        pixmap: &mut Pixmap,
        fr: &Frame,
        run: &[u32],
        s: &mut Scratch,
    ) {
        let fade = (fr.ctx.zoom - SHADOW_ZOOM).clamp(0.0, 1.0);
        let mut color = self.theme.shadow();
        color.set_alpha(color.alpha() * fade);
        // Short, capped shadows: enough to give depth without hiding streets.
        let max_len = 40.0 * fr.ctx.scale as f64;
        let clip = fr.rect(max_len + 2.0);
        let mut pb = PathBuilder::new();
        for &id in run {
            let f = &self.map.features[id as usize];
            let height = f.height;
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

    /// Objects in a flat (straight-down or footprint-only) view.
    pub(super) fn draw_objects_flat(
        &self,
        pixmap: &mut Pixmap,
        fr: &Frame,
        run: &[u32],
        s: &mut Scratch,
    ) {
        let split = run.partition_point(|&id| !self.map.features[id as usize].kind.is_point());
        let (solids, points) = run.split_at(split);
        if fr.ctx.zoom >= SHADOW_ZOOM {
            self.draw_shadows(pixmap, fr, solids, s);
        }
        self.draw_areas(pixmap, fr, solids, s);
        self.draw_props_flat(pixmap, fr, points);
    }

    /// One solid in a tilted view, built like the real thing:
    ///
    /// - walls from its base to its eaves (both true heights above the
    ///   ground, so a `building:part` with `min_height` floats where it is
    ///   mapped), shaded by orientation, with floors as window bands;
    /// - a roof of its mapped shape (flat, gabled, hipped, pyramidal,
    ///   skillion, dome) and colour, shaded per face under one sun.
    ///
    /// Only walls facing the viewer are drawn; roof faces are culled and
    /// painted back to front within the building.
    pub(super) fn draw_solid(&self, pixmap: &mut Pixmap, fr: &Frame, f: &Feature, s: &mut Scratch) {
        let detail = self.map.detail(f);
        let (mut base, top) = (f.base, f.height);
        if f.kind == Kind::Canopy {
            base = base.max(top - CANOPY_SLAB);
        }
        let mpu = fr.meters_per_unit();
        let outer = self.map.ring(f.ring_start);
        // Anchor: the outer ring's vertex average, in local units.
        let n = outer.len() as f64;
        let (ax, ay) = outer
            .iter()
            .fold((0.0, 0.0), |(x, y), p| (x + p[0] as f64, y + p[1] as f64));
        let anchor = [ax / n, ay / n];
        let footprint: Vec<[f64; 2]> = outer
            .iter()
            .map(|p| {
                [
                    (p[0] as f64 - anchor[0]) * mpu,
                    (p[1] as f64 - anchor[1]) * mpu,
                ]
            })
            .collect();
        let roof = roof_plan(f, &detail, &footprint, base, top);
        let eave = roof.as_ref().map_or(top, |r| r.eave);

        let roof_colour = detail.roof_colour.map(|c| self.theme.mapped_colour(c));
        let facade_colour = detail.facade_colour;
        let facade = |light: f32| match facade_colour {
            Some(c) => self.theme.mapped_facade(c, light),
            None => self.theme.facade(f.kind, light),
        };

        let base_px = base as f64 * fr.lift_per_m;
        let eave_px = eave as f64 * fr.lift_per_m;
        let top_px = top as f64 * fr.lift_per_m;
        let mut clip = fr.rect(2.0);
        clip.max_y += top_px + 2.0;
        let bbox = fr.bbox(f);
        let inside =
            clip.contains([bbox.min_x, bbox.min_y]) && clip.contains([bbox.max_x, bbox.max_y]);
        let light = {
            let (x, y) = (-1.0_f64, 0.5_f64);
            let len = (x * x + y * y).sqrt();
            [x / len, y / len]
        };
        let skillion = matches!(
            roof,
            Some(RoofPlan {
                shape: RoofShape::Skillion,
                ..
            })
        );

        // Floors, for window bands on buildings.
        let wall_m = (eave - base) as f64;
        let floors = if detail.levels >= 1.0 {
            detail.levels as f64
        } else {
            (wall_m / 3.2).round().max(1.0)
        };
        let floor_px = (eave_px - base_px) / floors;
        // Window bands fade in as floors grow from 2 to 4 pixels tall.
        let window_alpha = ((floor_px - 2.0) / 2.0).clamp(0.0, 1.0) as f32;
        let windows = f.kind == Kind::Building && window_alpha > 0.0 && wall_m >= 2.5;

        let mut walls: [Option<PathBuilder>; SHADES] = Default::default();
        let mut panes: [Option<PathBuilder>; SHADES] = Default::default();
        let mut lit_panes = PathBuilder::new();
        let mut flat_roof = PathBuilder::new();
        let mut posts: Vec<Point> = Vec::new();
        for (r, ring) in self.map.rings(f).enumerate() {
            fr.project(ring, &mut s.pts);
            if !inside {
                clip_ring(&mut s.pts, &clip, &mut s.tmp);
            }
            let n = s.pts.len();
            if n < 3 {
                continue;
            }
            if !skillion {
                for e in 0..n {
                    let (a, b) = (s.pts[e], s.pts[(e + 1) % n]);
                    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                    // Rings keep the solid on their left, so the outward
                    // normal is (dy, -dx): the wall faces the viewer when
                    // dx < 0.
                    if dx >= 0.0 || eave_px - base_px < 0.5 {
                        continue;
                    }
                    let len = (dx * dx + dy * dy).sqrt();
                    let lit = ((dy * light[0] - dx * light[1]) / len).max(0.0);
                    let shade = ((lit * SHADES as f64) as usize).min(SHADES - 1);
                    let quad = [
                        [a[0], a[1] - base_px],
                        [b[0], b[1] - base_px],
                        [b[0], b[1] - eave_px],
                        [a[0], a[1] - eave_px],
                    ];
                    push_ring(walls[shade].get_or_insert_with(PathBuilder::new), &quad);
                    if windows {
                        for i in 0..floors as usize {
                            let z0 = base_px + (i as f64 + 0.32) * floor_px;
                            let z1 = base_px + (i as f64 + 0.78) * floor_px;
                            let band = [
                                [a[0], a[1] - z0],
                                [b[0], b[1] - z0],
                                [b[0], b[1] - z1],
                                [a[0], a[1] - z1],
                            ];
                            if self.theme.night() && window_lit(f, e, i) {
                                push_ring(&mut lit_panes, &band);
                            } else {
                                push_ring(panes[shade].get_or_insert_with(PathBuilder::new), &band);
                            }
                        }
                    }
                }
            }
            if f.kind == Kind::Canopy && r == 0 && base_px >= 2.0 {
                let step = (n / 6).max(1);
                posts.extend(s.pts.iter().step_by(step).copied());
            }
            if roof.is_none() {
                s.tmp.clear();
                s.tmp.extend(s.pts.iter().map(|p| [p[0], p[1] - top_px]));
                push_ring(&mut flat_roof, &s.tmp);
            }
        }
        if !posts.is_empty() {
            let mut pb = PathBuilder::new();
            for p in &posts {
                pb.move_to(p[0] as f32, p[1] as f32);
                pb.line_to(p[0] as f32, (p[1] - base_px) as f32);
            }
            if let Some(path) = pb.finish() {
                let st = Stroke {
                    width: (0.3 * fr.ctx.ppm).max(1.0),
                    line_cap: LineCap::Butt,
                    ..Stroke::default()
                };
                let p = paint(self.theme.facade(f.kind, 0.0));
                pixmap.stroke_path(&path, &p, &st, Transform::identity(), None);
            }
        }
        for (i, (wall, pane)) in walls.into_iter().zip(panes).enumerate() {
            let light = i as f32 / (SHADES - 1) as f32;
            if let Some(path) = wall.and_then(PathBuilder::finish) {
                let p = paint(facade(light));
                pixmap.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
            }
            if let Some(path) = pane.and_then(PathBuilder::finish) {
                let mut glass = self.theme.window(facade(light), false);
                glass.set_alpha(glass.alpha() * window_alpha);
                let p = paint(glass);
                pixmap.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
            }
        }
        if let Some(path) = lit_panes.finish() {
            let mut glow = self.theme.window(facade(1.0), true);
            glow.set_alpha(glow.alpha() * window_alpha);
            let p = paint(glow);
            pixmap.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
        }

        let roof_fill = roof_colour.unwrap_or_else(|| match roof {
            Some(_) => self.theme.pitched_roof(),
            None => self.theme.roof(f.kind),
        });
        match roof {
            None => {
                if let Some(path) = flat_roof.finish() {
                    let p = paint(roof_fill);
                    pixmap.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
                    if fr.ctx.zoom >= 16.0 {
                        let spec = StrokeSpec {
                            color: self.theme.building_outline(),
                            width: 0.6 * fr.ctx.scale,
                            dash: None,
                            round: true,
                        };
                        let p = paint(spec.color);
                        pixmap.stroke_path(
                            &path,
                            &p,
                            &stroke(&spec, 0.0),
                            Transform::identity(),
                            None,
                        );
                    }
                }
            }
            Some(plan) => {
                let mut mesh = Mesh::new();
                plan.build(&mut mesh, &footprint, base, roof_fill, facade(0.8));
                mesh.draw(pixmap, fr, fr.apply(anchor[0], anchor[1]));
            }
        }
    }
}

/// Deterministic "is this window lit at night" for a building, wall edge
/// and floor: the same in every tile.
fn window_lit(f: &Feature, edge: usize, floor: usize) -> bool {
    let mut h = (f.ring_start as u64) << 32 ^ (edge as u64) << 16 ^ floor as u64;
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h % 5 < 2
}

/// A pitched roof over a footprint (meters around the anchor).
pub(crate) struct RoofPlan {
    shape: RoofShape,
    pub(crate) eave: f32,
    top: f32,
    /// Center, ridge axis and half-extents of the footprint's minimum-area
    /// bounding rectangle, along and across the ridge.
    center: [f64; 2],
    axis: [f64; 2],
    along: f64,
    across: f64,
}

/// Minimum-area oriented rectangle of a polygon: (center, axis, half-length
/// along axis, half-width across), with the axis along the longer side.
fn oriented_box(pts: &[[f64; 2]]) -> ([f64; 2], [f64; 2], f64, f64) {
    let n = pts.len();
    let mut best = (f64::INFINITY, [0.0, 0.0], [1.0, 0.0], 0.0, 0.0);
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-9 {
            continue;
        }
        let u = [dx / len, dy / len];
        let v = [-u[1], u[0]];
        let (mut u0, mut u1, mut v0, mut v1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for p in pts {
            let (pu, pv) = (p[0] * u[0] + p[1] * u[1], p[0] * v[0] + p[1] * v[1]);
            u0 = u0.min(pu);
            u1 = u1.max(pu);
            v0 = v0.min(pv);
            v1 = v1.max(pv);
        }
        let area = (u1 - u0) * (v1 - v0);
        if area < best.0 {
            let (cu, cv) = ((u0 + u1) / 2.0, (v0 + v1) / 2.0);
            let center = [u[0] * cu + v[0] * cv, u[1] * cu + v[1] * cv];
            best = (area, center, u, (u1 - u0) / 2.0, (v1 - v0) / 2.0);
        }
    }
    let (_, c, u, hu, hv) = best;
    if hu >= hv {
        (c, u, hu, hv)
    } else {
        (c, [-u[1], u[0]], hv, hu)
    }
}

pub(crate) fn roof_plan(
    f: &Feature,
    d: &Detail,
    footprint: &[[f64; 2]],
    base: f32,
    top: f32,
) -> Option<RoofPlan> {
    // Pitched roofs need a single, simple outline.
    if d.roof == RoofShape::Flat
        || f.kind != Kind::Building
        || f.ring_count != 1
        || footprint.len() < 3
    {
        return None;
    }
    let (center, mut axis, mut along, mut across) = oriented_box(footprint);
    if d.across {
        axis = [-axis[1], axis[0]];
        std::mem::swap(&mut along, &mut across);
    }
    let wall = top - base;
    let auto = match d.roof {
        RoofShape::Dome => across.min(along) as f32,
        RoofShape::Skillion => (across * 0.5) as f32,
        _ => (across.min(along) * 0.6) as f32,
    };
    // A mapped roof height is honored in full (spires are often a whole
    // part that is all roof); an estimated one stays modest.
    let rise = if d.roof_height > 0.0 {
        d.roof_height.min(wall)
    } else {
        auto.min(wall * 0.6)
    }
    .max(0.0);
    if rise < 0.3 {
        return None;
    }
    Some(RoofPlan {
        shape: d.roof,
        eave: top - rise,
        top,
        center,
        axis,
        along,
        across,
    })
}

impl RoofPlan {
    pub(crate) fn shape(&self) -> RoofShape {
        self.shape
    }

    pub(crate) fn build(
        &self,
        mesh: &mut Mesh,
        footprint: &[[f64; 2]],
        base: f32,
        roof: Color,
        gable: Color,
    ) {
        let (eave, top) = (self.eave as f64, self.top as f64);
        let c = self.center;
        let inside = [c[0], c[1], (eave + base as f64) / 2.0];
        let n = footprint.len();
        match self.shape {
            RoofShape::Gabled | RoofShape::Hipped | RoofShape::Pyramidal => {
                let half = match self.shape {
                    RoofShape::Gabled => self.along,
                    RoofShape::Hipped => (self.along - self.across).max(0.0),
                    _ => 0.0,
                };
                let u = self.axis;
                // Each eave edge rises to its projection on the ridge.
                let proj = |p: [f64; 2]| -> [f64; 2] {
                    let t = ((p[0] - c[0]) * u[0] + (p[1] - c[1]) * u[1]).clamp(-half, half);
                    [c[0] + u[0] * t, c[1] + u[1] * t]
                };
                for i in 0..n {
                    let (a, b) = (footprint[i], footprint[(i + 1) % n]);
                    let (pa, pb) = (proj(a), proj(b));
                    let mut face =
                        vec![[a[0], a[1], eave], [b[0], b[1], eave], [pb[0], pb[1], top]];
                    if (pa[0] - pb[0]).abs() + (pa[1] - pb[1]).abs() > 1e-6 {
                        face.push([pa[0], pa[1], top]);
                    }
                    // Near-vertical faces are gable walls, not roof.
                    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
                    let pm = proj(mid);
                    let run = ((mid[0] - pm[0]).powi(2) + (mid[1] - pm[1]).powi(2)).sqrt();
                    let colour = if run < 0.05 * self.across.max(0.1) {
                        gable
                    } else {
                        roof
                    };
                    mesh.face_outward(face, inside, colour);
                }
            }
            RoofShape::Skillion => {
                // One plane rising across the building.
                let v = [-self.axis[1], self.axis[0]];
                let w = self.across.max(1e-6);
                let z = |p: [f64; 2]| {
                    let t = (((p[0] - c[0]) * v[0] + (p[1] - c[1]) * v[1]) + w) / (2.0 * w);
                    eave + (top - eave) * t.clamp(0.0, 1.0)
                };
                for i in 0..n {
                    let (a, b) = (footprint[i], footprint[(i + 1) % n]);
                    let wall = vec![
                        [a[0], a[1], base as f64],
                        [b[0], b[1], base as f64],
                        [b[0], b[1], z(b)],
                        [a[0], a[1], z(a)],
                    ];
                    mesh.face_outward(wall, inside, gable);
                }
                let roof_face: Vec<_> = footprint.iter().map(|&p| [p[0], p[1], z(p)]).collect();
                mesh.face_outward(roof_face, inside, roof);
            }
            RoofShape::Dome => {
                const RINGS: usize = 6;
                let level = |k: usize| {
                    let phi = std::f64::consts::FRAC_PI_2 * k as f64 / RINGS as f64;
                    (phi.cos(), eave + (top - eave) * phi.sin())
                };
                for k in 0..RINGS {
                    let ((s0, z0), (s1, z1)) = (level(k), level(k + 1));
                    let at = |p: [f64; 2], s: f64, z: f64| {
                        [c[0] + (p[0] - c[0]) * s, c[1] + (p[1] - c[1]) * s, z]
                    };
                    for i in 0..n {
                        let (a, b) = (footprint[i], footprint[(i + 1) % n]);
                        let quad = vec![at(a, s0, z0), at(b, s0, z0), at(b, s1, z1), at(a, s1, z1)];
                        mesh.face_outward(quad, inside, roof);
                    }
                }
            }
            RoofShape::Flat => {}
        }
    }
}
