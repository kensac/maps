//! A tiny 3D mesh rasterizer for the oblique/orthographic camera.
//!
//! Geometry is built in real meters relative to an anchor point on the map:
//! `[east, south, up]`. Faces are projected through the camera, back faces
//! are culled against the true view direction, every face is shaded by its
//! normal under one sun, and faces are painted back to front. Roofs and the
//! bespoke object models (benches, lamps, hydrants...) are all made of these.

use super::camera::Frame;
use super::paint::paint;
use tiny_skia::{Color, FillRule, LineCap, PathBuilder, Pixmap, Rect, Stroke, Transform};

pub(super) type V3 = [f64; 3];

/// Direction toward the sun: from the west-south-west, fairly high.
const SUN: V3 = [-0.55, 0.3, 0.78];
const AMBIENT: f32 = 0.58;

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: V3) -> V3 {
    let l = dot(v, v).sqrt();
    if l == 0.0 {
        v
    } else {
        [v[0] / l, v[1] / l, v[2] / l]
    }
}

/// Scales a color's RGB by `k` (keeping alpha).
pub(super) fn shade(c: Color, k: f32) -> Color {
    Color::from_rgba(
        (c.red() * k).min(1.0),
        (c.green() * k).min(1.0),
        (c.blue() * k).min(1.0),
        c.alpha(),
    )
    .unwrap_or(c)
}

enum Prim {
    Face {
        pts: Vec<V3>,
        color: Color,
    },
    Ball {
        center: V3,
        radius: f64,
        color: Color,
        highlight: Color,
    },
    Beam {
        a: V3,
        b: V3,
        width: f64,
        color: Color,
    },
}

/// Collects primitives of one object, then draws them in depth order.
pub(super) struct Mesh {
    prims: Vec<Prim>,
    /// Whether faces are lit by the sun (off for glowing parts).
    pub(super) lit: bool,
}

impl Mesh {
    pub(super) fn new() -> Self {
        Mesh {
            prims: Vec::new(),
            lit: true,
        }
    }

    /// A planar face. Its winding decides its outward side: the outward
    /// normal is `-(p1 - p0) × (p2 - p0)` in this [east, south, up] frame.
    /// Back faces are culled. See [`Mesh::face_outward`] to orient faces.
    pub(super) fn face(&mut self, pts: Vec<V3>, color: Color) {
        if pts.len() >= 3 {
            self.prims.push(Prim::Face { pts, color });
        }
    }

    /// A face oriented so its outward side points away from `inside`.
    pub(super) fn face_outward(&mut self, mut pts: Vec<V3>, inside: V3, color: Color) {
        if pts.len() < 3 {
            return;
        }
        let n = cross(sub(pts[1], pts[0]), sub(pts[2], pts[0]));
        let outward = [-n[0], -n[1], -n[2]];
        if dot(outward, sub(pts[0], inside)) < 0.0 {
            pts.reverse();
        }
        self.face(pts, color);
    }

    /// A vertical prism over `footprint` (counter-clockwise seen from above,
    /// in [east, south] meters) from `z0` to `z1`. `top` colors the cap.
    pub(super) fn prism(
        &mut self,
        footprint: &[[f64; 2]],
        z0: f64,
        z1: f64,
        side: Color,
        top: Color,
    ) {
        let n = footprint.len();
        if n < 3 || z1 <= z0 {
            return;
        }
        // Make the footprint clockwise in (east, south) = counter-clockwise
        // seen from above with north up.
        let mut ring = footprint.to_vec();
        let area: f64 = (0..n)
            .map(|i| {
                let (a, b) = (ring[i], ring[(i + 1) % n]);
                a[0] * b[1] - b[0] * a[1]
            })
            .sum();
        if area > 0.0 {
            ring.reverse();
        }
        for i in 0..n {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            self.face(
                vec![
                    [a[0], a[1], z0],
                    [b[0], b[1], z0],
                    [b[0], b[1], z1],
                    [a[0], a[1], z1],
                ],
                side,
            );
        }
        self.face(ring.iter().map(|p| [p[0], p[1], z1]).collect(), top);
    }

    /// An axis-aligned box centered at (`x`, `y`) on the ground plane.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn cuboid(
        &mut self,
        x: f64,
        y: f64,
        w: f64,
        d: f64,
        z0: f64,
        z1: f64,
        side: Color,
        top: Color,
    ) {
        let (hw, hd) = (w / 2.0, d / 2.0);
        let fp = [
            [x - hw, y - hd],
            [x + hw, y - hd],
            [x + hw, y + hd],
            [x - hw, y + hd],
        ];
        self.prism(&fp, z0, z1, side, top);
    }

    /// A cylinder approximated by `sides` facets.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn cylinder(
        &mut self,
        x: f64,
        y: f64,
        r: f64,
        z0: f64,
        z1: f64,
        side: Color,
        top: Color,
        sides: usize,
    ) {
        let fp: Vec<[f64; 2]> = (0..sides)
            .map(|i| {
                let t = std::f64::consts::TAU * i as f64 / sides as f64;
                [x + r * t.cos(), y + r * t.sin()]
            })
            .collect();
        self.prism(&fp, z0, z1, side, top);
    }

    /// A cone (or pyramid with few sides) standing on the ground plane.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn cone(
        &mut self,
        x: f64,
        y: f64,
        r: f64,
        z0: f64,
        z1: f64,
        color: Color,
        sides: usize,
    ) {
        let apex = [x, y, z1];
        for i in 0..sides {
            let t0 = std::f64::consts::TAU * i as f64 / sides as f64;
            let t1 = std::f64::consts::TAU * (i + 1) as f64 / sides as f64;
            let a = [x + r * t0.cos(), y + r * t0.sin(), z0];
            let b = [x + r * t1.cos(), y + r * t1.sin(), z0];
            // Counter-clockwise from outside: a → apex → b (y points south).
            self.face(vec![a, apex, b], color);
        }
    }

    /// A sphere: drawn as a shaded disc (exact for an orthographic camera).
    pub(super) fn ball(&mut self, center: V3, radius: f64, color: Color, highlight: Color) {
        self.prims.push(Prim::Ball {
            center,
            radius,
            color,
            highlight,
        });
    }

    /// A thin bar between two points, `width` meters thick.
    pub(super) fn beam(&mut self, a: V3, b: V3, width: f64, color: Color) {
        self.prims.push(Prim::Beam { a, b, width, color });
    }

    /// Turns the whole mesh to face compass `heading` (radians): models are
    /// built facing south (+y), the side a sign face or a lamp arm is on.
    pub(super) fn turn_to(&mut self, heading: f64) {
        if !heading.is_finite() {
            return;
        }
        let a = heading - std::f64::consts::PI;
        let (c, s) = (a.cos(), a.sin());
        let r = |p: &mut V3| *p = [p[0] * c - p[1] * s, p[0] * s + p[1] * c, p[2]];
        for prim in &mut self.prims {
            match prim {
                Prim::Face { pts, .. } => pts.iter_mut().for_each(r),
                Prim::Ball { center, .. } => r(center),
                Prim::Beam { a, b, .. } => {
                    r(a);
                    r(b);
                }
            }
        }
    }

    /// Projects, culls, shades, sorts and draws the mesh anchored at screen
    /// point `at` (the anchor's ground position).
    pub(super) fn draw(self, pixmap: &mut Pixmap, fr: &Frame, at: [f64; 2]) {
        let view = fr.view_dir();
        let sun = normalize(SUN);
        let project = |p: V3| fr.meters_to_screen(at, p);
        let depth = |p: V3| fr.closeness(p);
        let mut items: Vec<(f64, usize)> = Vec::with_capacity(self.prims.len());
        for (i, prim) in self.prims.iter().enumerate() {
            let d = match prim {
                Prim::Face { pts, .. } => {
                    let n = pts.len() as f64;
                    let c = pts
                        .iter()
                        .fold([0.0; 3], |s, p| [s[0] + p[0], s[1] + p[1], s[2] + p[2]]);
                    depth([c[0] / n, c[1] / n, c[2] / n])
                }
                Prim::Ball { center, .. } => depth(*center),
                Prim::Beam { a, b, .. } => depth([
                    (a[0] + b[0]) / 2.0,
                    (a[1] + b[1]) / 2.0,
                    (a[2] + b[2]) / 2.0,
                ]),
            };
            items.push((d, i));
        }
        items.sort_by(|a, b| a.0.total_cmp(&b.0));
        let ppm = fr.ctx.ppm as f64;
        for (_, i) in items {
            match &self.prims[i] {
                Prim::Face { pts, color } => {
                    let normal = normalize(cross(sub(pts[1], pts[0]), sub(pts[2], pts[0])));
                    // Faces are counter-clockwise from outside in a
                    // left-handed frame (y south), so the outward normal is
                    // the negated cross product.
                    let outward = [-normal[0], -normal[1], -normal[2]];
                    if dot(outward, view) <= 1e-6 {
                        continue;
                    }
                    let k = if self.lit {
                        AMBIENT + (1.0 - AMBIENT) * dot(outward, sun).max(0.0) as f32
                    } else {
                        1.0
                    };
                    let mut pb = PathBuilder::new();
                    for (j, p) in pts.iter().enumerate() {
                        let s = project(*p);
                        if j == 0 {
                            pb.move_to(s[0] as f32, s[1] as f32);
                        } else {
                            pb.line_to(s[0] as f32, s[1] as f32);
                        }
                    }
                    pb.close();
                    if let Some(path) = pb.finish() {
                        let p = paint(shade(*color, k));
                        pixmap.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
                    }
                }
                Prim::Ball {
                    center,
                    radius,
                    color,
                    highlight,
                } => {
                    let c = project(*center);
                    let r = (radius * ppm).max(0.8);
                    disc(pixmap, c, r, *color);
                    if r >= 3.0 {
                        disc(
                            pixmap,
                            [c[0] - r * 0.3, c[1] - r * 0.3],
                            r * 0.45,
                            *highlight,
                        );
                    }
                }
                Prim::Beam { a, b, width, color } => {
                    let (sa, sb) = (project(*a), project(*b));
                    let mut pb = PathBuilder::new();
                    pb.move_to(sa[0] as f32, sa[1] as f32);
                    pb.line_to(sb[0] as f32, sb[1] as f32);
                    if let Some(path) = pb.finish() {
                        let st = Stroke {
                            width: (width * ppm).max(0.8) as f32,
                            line_cap: LineCap::Butt,
                            ..Stroke::default()
                        };
                        pixmap.stroke_path(&path, &paint(*color), &st, Transform::identity(), None);
                    }
                }
            }
        }
    }
}

fn disc(pixmap: &mut Pixmap, c: [f64; 2], r: f64, color: Color) {
    if let Some(rect) = Rect::from_ltrb(
        (c[0] - r) as f32,
        (c[1] - r) as f32,
        (c[0] + r) as f32,
        (c[1] + r) as f32,
    ) {
        let mut pb = PathBuilder::new();
        pb.push_oval(rect);
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
}
