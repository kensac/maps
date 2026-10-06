//! Line features: roads, rail, waterways and overlays.
//!
//! Lines on the ground draw immediately. In 3D views, lines that stand above
//! the ground become scene chunks, painted in depth order with everything
//! else that has height. Every raised line carries a height per vertex:
//! - bridges and viaducts are decks on pillars, and the roads and rails
//!   leading up to them are ramps on embankments, following the smoothed
//!   profiles from [`crate::elevation`] (no steps at abutments);
//! - walls, hedges, fences and dams are vertical faces topped by their line;
//! - power lines, aerial tramways and gantries are strung between poles;
//! - tree rows become individual trees.

use super::camera::Frame;
use super::paint::{paint, push_ring, stroke_pieces, stroke_pieces_with, LineGeom};
use super::scene::{Form, RaisedLine, Scene, V3};
use super::{Renderer, Scratch};
use crate::classify::{Group, Kind};
use crate::elevation::{deck_height, METERS_PER_LAYER};
use crate::geo::{clip_polyline, signed_area2, Piece, Rect};
use crate::map::Feature;
use crate::style::{LineStyle, StrokeSpec};
use tiny_skia::{Color, FillRule, LineCap, PathBuilder, Pixmap, Stroke, Transform};

/// Distance between trees in a tree row, in meters.
const TREE_SPACING: f64 = 7.0;
/// Distance between bridge pillars, in meters.
const PILLAR_SPACING: f64 = 35.0;

fn solid(color: Color, width: f32) -> StrokeSpec {
    StrokeSpec {
        color,
        width,
        dash: None,
        round: false,
    }
}

/// Calls `f` with the position and height at every multiple of `spacing`
/// along `pts` (which start `distance` pixels along their line, so positions
/// agree across tiles).
fn along(pts: &[V3], distance: f64, spacing: f64, mut f: impl FnMut(V3)) {
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
            f([
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            ]);
            next += spacing;
        }
        dist += len;
    }
}

/// Strokes vertical segments from each ground point up by its height.
fn verticals(pixmap: &mut Pixmap, points: &[V3], color: Color, width: f32) {
    let mut pb = PathBuilder::new();
    for p in points {
        pb.move_to(p[0] as f32, p[1] as f32);
        pb.line_to(p[0] as f32, (p[1] - p[2]) as f32);
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

/// The polyline at each point's own height, minus `drop` pixels.
fn lifted(pts: &[V3], drop: f64, distance: f64) -> LineGeom {
    LineGeom {
        points: pts.iter().map(|p| [p[0], p[1] - p[2] + drop]).collect(),
        pieces: vec![Piece {
            start: 0,
            len: pts.len(),
            distance,
        }],
    }
}

/// Fills the vertical faces between `bottom` pixels and each point's height
/// along the line. Quads are oriented alike so overlaps at bends union.
fn curtain(pixmap: &mut Pixmap, pts: &[V3], bottom: f64, color: Color) {
    let mut pb = PathBuilder::new();
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let mut quad = [
            [a[0], a[1] - bottom.min(a[2])],
            [b[0], b[1] - bottom.min(b[2])],
            [b[0], b[1] - b[2]],
            [a[0], a[1] - a[2]],
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

/// Liang–Barsky clipping of a polyline with a height per vertex; heights
/// are interpolated at cut points. Returns pieces with their start distance.
fn clip_raised(pts: &[V3], rect: &Rect) -> Vec<(Vec<V3>, f64)> {
    let mut out: Vec<(Vec<V3>, f64)> = Vec::new();
    let mut open = false;
    let mut distance = 0.0;
    let lerp = |a: V3, b: V3, t: f64| {
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    };
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let d = [b[0] - a[0], b[1] - a[1]];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
        let (mut t0, mut t1, mut visible) = (0.0_f64, 1.0_f64, true);
        for (p, q) in [
            (-d[0], a[0] - rect.min_x),
            (d[0], rect.max_x - a[0]),
            (-d[1], a[1] - rect.min_y),
            (d[1], rect.max_y - a[1]),
        ] {
            if p == 0.0 {
                visible &= q >= 0.0;
            } else {
                let t = q / p;
                if p < 0.0 {
                    t0 = t0.max(t);
                } else {
                    t1 = t1.min(t);
                }
            }
        }
        if visible && t0 <= t1 {
            if !open || t0 > 0.0 {
                out.push((vec![lerp(a, b, t0)], distance + t0 * len));
                open = true;
            }
            if let Some(last) = out.last_mut() {
                last.0.push(lerp(a, b, t1));
            }
            if t1 < 1.0 {
                open = false;
            }
        } else {
            open = false;
        }
        distance += len;
    }
    out.retain(|(p, _)| p.len() >= 2);
    out
}

impl Renderer<'_> {
    /// How a line kind stands above the ground on its own, and how high
    /// (meters). Roads and rails get their heights per vertex instead.
    fn raised_form(&self, kind: Kind, height: f32) -> Option<(Form, f64)> {
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
                Some((Form::Face(self.theme.barrier_face(kind)?), h))
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
            let widest = [&style.casing, &style.line]
                .iter()
                .filter_map(|s| s.as_ref().map(|s| s.width))
                .fold(0.0_f32, f32::max);
            let tree_row = scene.is_some() && f0.kind == Kind::TreeRow;
            if widest <= 0.0 && !tree_row {
                continue;
            }
            let fixed = scene
                .as_ref()
                .and_then(|_| self.raised_form(f0.kind, f0.height))
                .map(|(form, m)| (form, m * fr.lift_per_m))
                .filter(|&(_, px)| px >= 1.0);
            let base_px = (f0.base as f64 * fr.lift_per_m).max(0.0);

            let mut geom = LineGeom::default();
            for &id in ids {
                let f = &self.map.features[id as usize];
                let Some(scene) = scene.as_deref_mut() else {
                    self.ground_geometry(fr, f, widest, 0.0, s, &mut geom);
                    continue;
                };
                // Roads and rails with an elevation profile.
                if let Some(elev) = self.map.elevations(f) {
                    if !self.raise_profile(fr, f, id, elev, &style, widest, scene) {
                        self.ground_geometry(fr, f, widest, 0.0, s, &mut geom);
                    }
                    continue;
                }
                // Bridges without a profile: a level deck.
                let deck = (f.group() == Group::TransportBridges).then(|| {
                    let m = f.layer.max(1) as f64 * METERS_PER_LAYER as f64;
                    let form = Form::Deck {
                        pillar_spacing: PILLAR_SPACING,
                    };
                    (form, m * fr.lift_per_m)
                });
                let raised = deck.or(fixed);
                if raised.is_none() && !tree_row {
                    self.ground_geometry(fr, f, widest, 0.0, s, &mut geom);
                    continue;
                }
                let raise = raised.map_or(0.0, |r| r.1);
                let mut tmp = LineGeom::default();
                self.ground_geometry(fr, f, widest, raise, s, &mut tmp);
                let pieces: Vec<(Vec<V3>, f64)> = tmp
                    .pieces
                    .iter()
                    .map(|p| {
                        let pts = tmp.points[p.start..p.start + p.len]
                            .iter()
                            .map(|q| [q[0], q[1], raise])
                            .collect();
                        (pts, p.distance)
                    })
                    .collect();
                if tree_row {
                    let spacing = TREE_SPACING * fr.ctx.ppm as f64;
                    for (pts, distance) in &pieces {
                        along(pts, *distance, spacing, |p| {
                            scene.add_tree([p[0], p[1]], f.height, id)
                        });
                    }
                } else if let Some((form, raise)) = raised {
                    let line = RaisedLine {
                        style: style.clone(),
                        form,
                        base: base_px.min(raise),
                    };
                    scene.add_line(line, &pieces, id, matches!(form, Form::Wire(_)));
                }
            }
            if !geom.pieces.is_empty() {
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

    /// Projects and clips a feature's lines on the ground, allowing for
    /// `raise` pixels of height above them.
    fn ground_geometry(
        &self,
        fr: &Frame,
        f: &Feature,
        widest: f32,
        raise: f64,
        s: &mut Scratch,
        geom: &mut LineGeom,
    ) {
        let mut clip = fr.rect(widest as f64 / 2.0 + 2.0);
        clip.max_y += raise;
        if !fr.bbox(f).intersects(&clip) {
            return;
        }
        for ring in self.map.rings(f) {
            fr.project(ring, &mut s.pts);
            if s.pts.len() >= 2 {
                clip_polyline(&s.pts, &clip, &mut geom.points, &mut geom.pieces);
            }
        }
    }

    /// Adds a road or rail following its elevation profile to the scene, as
    /// a deck where it is a bridge and a ramp on an embankment elsewhere.
    /// Returns false when it never leaves the ground in this view.
    #[allow(clippy::too_many_arguments)]
    fn raise_profile(
        &self,
        fr: &Frame,
        f: &Feature,
        id: u32,
        elev: &[f32],
        style: &LineStyle,
        widest: f32,
        scene: &mut Scene,
    ) -> bool {
        let top = elev.iter().copied().fold(0.0_f32, f32::max) as f64 * fr.lift_per_m;
        if top < 1.0 {
            return false;
        }
        let mut clip = fr.rect(widest as f64 / 2.0 + 2.0);
        clip.max_y += top;
        if !fr.bbox(f).intersects(&clip) {
            return true;
        }
        let pts: Vec<V3> = self
            .map
            .rings(f)
            .flatten()
            .zip(elev)
            .map(|(p, &e)| {
                let q = fr.apply(p[0] as f64, p[1] as f64);
                [q[0], q[1], e as f64 * fr.lift_per_m]
            })
            .collect();
        let pieces = clip_raised(&pts, &clip);
        let form = if deck_height(f.kind, f.layer, f.flags) > 0.0 {
            Form::Deck {
                pillar_spacing: PILLAR_SPACING,
            }
        } else {
            Form::Ramp(self.theme.bank())
        };
        let line = RaisedLine {
            style: style.clone(),
            form,
            base: 0.0,
        };
        scene.add_line(line, &pieces, id, false);
        true
    }

    /// Draws one chunk of a raised line: whatever holds it up first, then
    /// the line itself at its height.
    pub(super) fn draw_raised(
        &self,
        pixmap: &mut Pixmap,
        fr: &Frame,
        line: &RaisedLine,
        pts: &[V3],
        distance: f64,
    ) {
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
                let ground: Vec<V3> = pts.iter().map(|p| [p[0], p[1], 0.0]).collect();
                stroke_pieces(
                    pixmap,
                    &lifted(&ground, 0.0, distance),
                    &shade,
                    Transform::identity(),
                );
                let mut pillars = Vec::new();
                along(pts, distance, pillar_spacing * ppm, |p| {
                    if p[2] >= 3.0 {
                        pillars.push(p);
                    }
                });
                let width = (1.5 * ppm).max(1.0) as f32;
                verticals(pixmap, &pillars, self.theme.deck_side(), width);
                let thickness = (1.5 * fr.lift_per_m).max(1.0);
                let side = solid(self.theme.deck_side(), base_width);
                let underside = lifted(pts, thickness, distance);
                stroke_pieces_with(pixmap, &underside, &side, Transform::identity(), true);
            }
            Form::Ramp(color) => curtain(pixmap, pts, 0.0, color),
            Form::Face(color) => curtain(pixmap, pts, line.base, color),
            Form::Wire(color) => {
                let width = (0.6 * ppm).clamp(1.0, 4.0) as f32;
                let ends = [pts[0], pts[pts.len() - 1]];
                verticals(pixmap, &ends, color, width);
            }
        }
        let top = lifted(pts, 0.0, distance);
        for spec in [&line.style.casing, &line.style.line].into_iter().flatten() {
            stroke_pieces_with(pixmap, &top, spec, Transform::identity(), true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raised_clipping_interpolates_heights() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        let pieces = clip_raised(&[[-10.0, 5.0, 0.0], [10.0, 5.0, 20.0]], &r);
        assert_eq!(pieces.len(), 1);
        let (pts, dist) = &pieces[0];
        assert_eq!(pts[0], [0.0, 5.0, 10.0]);
        assert_eq!(pts[1], [10.0, 5.0, 20.0]);
        assert!((dist - 10.0).abs() < 1e-9);
    }
}
