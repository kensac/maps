//! Line features: roads, rail, waterways and overlays.
//!
//! Lines on the ground draw immediately. In 3D views, lines that stand above
//! the ground become scene chunks, painted in depth order with everything
//! else that has height:
//! - bridges and viaducts are decks raised by their layer, on pillars;
//! - walls, hedges, fences and dams are vertical faces topped by their line;
//! - power lines, aerial tramways and gantries are strung between poles;
//! - tree rows become individual trees.

use super::camera::Frame;
use super::paint::{paint, push_ring, stroke_pieces, stroke_pieces_with, LineGeom};
use super::scene::{Form, RaisedLine, Scene};
use super::{Renderer, Scratch, METERS_PER_LAYER};
use crate::classify::{Group, Kind};
use crate::geo::{clip_polyline, signed_area2, Piece, Point};
use crate::style::{LineStyle, StrokeSpec};
use tiny_skia::{Color, FillRule, LineCap, PathBuilder, Pixmap, Stroke, Transform};

/// Distance between trees in a tree row, in meters.
const TREE_SPACING: f64 = 7.0;

fn solid(color: Color, width: f32) -> StrokeSpec {
    StrokeSpec {
        color,
        width,
        dash: None,
        round: false,
    }
}

/// Calls `f` at every multiple of `spacing` along `pts`, which start
/// `distance` pixels along their line, so positions agree across tiles.
fn along(pts: &[Point], distance: f64, spacing: f64, mut f: impl FnMut(Point)) {
    if spacing <= 0.0 {
        return;
    }
    let mut dist = distance;
    let mut next = (dist / spacing).ceil() * spacing;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        while len > 0.0 && next <= dist + len {
            let t = (next - dist) / len;
            f([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
            next += spacing;
        }
        dist += len;
    }
}

/// Strokes vertical segments from `bottom` up to `top` pixels above points.
fn verticals(pixmap: &mut Pixmap, points: &[Point], top: f64, color: Color, width: f32) {
    let mut pb = PathBuilder::new();
    for p in points {
        pb.move_to(p[0] as f32, p[1] as f32);
        pb.line_to(p[0] as f32, (p[1] - top) as f32);
    }
    if let Some(path) = pb.finish() {
        let st = Stroke {
            width,
            line_cap: LineCap::Butt,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint(color), &st, Transform::identity(), None);
    }
}

fn geom_of(pts: &[Point], distance: f64) -> LineGeom {
    LineGeom {
        points: pts.to_vec(),
        pieces: vec![Piece {
            start: 0,
            len: pts.len(),
            distance,
        }],
    }
}

impl Renderer<'_> {
    /// How a line kind stands above the ground, and how high (meters).
    fn raised_form(&self, kind: Kind, group: Group, layer: i8, height: f32) -> Option<(Form, f64)> {
        if group == Group::TransportBridges {
            let deck = layer.max(1) as f64 * METERS_PER_LAYER;
            return Some((
                Form::Deck {
                    pillar_spacing: 35.0,
                },
                deck,
            ));
        }
        let h = height as f64;
        match kind {
            Kind::JetBridge => Some((
                Form::Deck {
                    pillar_spacing: 12.0,
                },
                h,
            )),
            Kind::Pipeline => Some((
                Form::Deck {
                    pillar_spacing: 8.0,
                },
                h,
            )),
            Kind::Hedge | Kind::Fence | Kind::Wall | Kind::LowBarrier | Kind::Dam | Kind::Weir => {
                let color = self.theme.barrier_face(kind)?;
                Some((Form::Face(color), h))
            }
            Kind::PowerLine | Kind::Aerialway | Kind::Gantry => {
                Some((Form::Wire(self.theme.wire(kind)), h))
            }
            _ => None,
        }
    }

    /// Lines draw in two passes over the run: every casing, then every line,
    /// so that casings never cut across crossing roads of the same layer.
    pub(super) fn draw_lines(
        &self,
        pixmap: &mut Pixmap,
        fr: &Frame,
        run: &[u32],
        s: &mut Scratch,
        mut scene: Option<&mut Scene>,
    ) {
        let mut ground: Vec<(LineStyle, LineGeom)> = Vec::new();
        for ids in self.batches(run) {
            let f0 = &self.map.features[ids[0] as usize];
            let style = self.theme.line(f0.kind, f0.flags, f0.height, &fr.ctx);
            let raised = scene
                .as_ref()
                .and_then(|_| self.raised_form(f0.kind, f0.group(), f0.layer, f0.height))
                .map(|(form, m)| (form, m * fr.lift_per_m))
                .filter(|&(_, px)| px >= 1.0);
            let tree_row = scene.is_some() && f0.kind == Kind::TreeRow;
            let widest = [&style.casing, &style.line]
                .iter()
                .filter_map(|s| s.as_ref().map(|s| s.width))
                .fold(0.0_f32, f32::max);
            if widest <= 0.0 && !tree_row {
                continue;
            }
            let lift = raised.map_or(0.0, |r| r.1);
            let mut clip = fr.rect(widest as f64 / 2.0 + 2.0);
            clip.max_y += lift;
            let mut geom = LineGeom::default();
            for &id in ids {
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
            if geom.pieces.is_empty() {
                continue;
            }
            let pieces = || {
                geom.pieces
                    .iter()
                    .map(|p| (geom.points[p.start..p.start + p.len].to_vec(), p.distance))
                    .collect::<Vec<_>>()
            };
            if tree_row {
                if let Some(scene) = scene.as_deref_mut() {
                    let spacing = TREE_SPACING * fr.ctx.ppm as f64;
                    for (pts, distance) in pieces() {
                        along(&pts, distance, spacing, |p| {
                            scene.add_tree(p, f0.height, ids[0])
                        });
                    }
                }
            } else if let (Some((form, raise)), Some(scene)) = (raised, scene.as_deref_mut()) {
                let per_segment = matches!(form, Form::Wire(_));
                let base = (f0.base as f64 * fr.lift_per_m).min(raise);
                let line = RaisedLine {
                    style,
                    form,
                    raise,
                    base,
                };
                scene.add_line(line, &pieces(), ids[0], per_segment);
            } else {
                ground.push((style, geom));
            }
        }
        for pass in 0..2 {
            for (style, geom) in &ground {
                let spec = if pass == 0 {
                    &style.casing
                } else {
                    &style.line
                };
                if let Some(spec) = spec {
                    stroke_pieces(pixmap, geom, spec, Transform::identity());
                }
            }
        }
    }

    /// Draws one chunk of a raised line: whatever holds it up first, then
    /// the line itself at its height.
    pub(super) fn draw_raised(
        &self,
        pixmap: &mut Pixmap,
        fr: &Frame,
        line: &RaisedLine,
        pts: &[Point],
        distance: f64,
    ) {
        let geom = geom_of(pts, distance);
        let ppm = fr.ctx.ppm as f64;
        let base_width = line
            .style
            .casing
            .as_ref()
            .or(line.style.line.as_ref())
            .map_or(1.0, |s| s.width);
        match line.form {
            Form::Deck { pillar_spacing } => {
                // Shade directly beneath the deck.
                let shade = solid(self.theme.shadow(), base_width);
                stroke_pieces(pixmap, &geom, &shade, Transform::identity());
                if line.raise >= 4.0 {
                    let mut pillars = Vec::new();
                    along(pts, distance, pillar_spacing * ppm, |p| pillars.push(p));
                    let width = (1.5 * ppm).max(1.0) as f32;
                    verticals(pixmap, &pillars, line.raise, self.theme.deck_side(), width);
                }
                let thickness = (1.5 * fr.lift_per_m).max(1.0);
                let side = solid(self.theme.deck_side(), base_width);
                let bottom = Transform::from_translate(0.0, (thickness - line.raise) as f32);
                stroke_pieces_with(pixmap, &geom, &side, bottom, true);
            }
            Form::Face(color) => {
                let mut pb = PathBuilder::new();
                for w in pts.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    let (z0, z1) = (line.base, line.raise);
                    let mut quad = [
                        [a[0], a[1] - z0],
                        [b[0], b[1] - z0],
                        [b[0], b[1] - z1],
                        [a[0], a[1] - z1],
                    ];
                    if signed_area2(&quad) < 0.0 {
                        quad.reverse();
                    }
                    push_ring(&mut pb, &quad);
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
            Form::Wire(color) => {
                let width = (0.6 * ppm).clamp(1.0, 4.0) as f32;
                let ends = [pts[0], pts[pts.len() - 1]];
                verticals(pixmap, &ends, line.raise, color, width);
            }
        }
        let raised = Transform::from_translate(0.0, -line.raise as f32);
        for spec in [&line.style.casing, &line.style.line].into_iter().flatten() {
            stroke_pieces_with(pixmap, &geom, spec, raised, true);
        }
    }
}
