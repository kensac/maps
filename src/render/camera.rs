//! The camera: an orthographic view with bearing and pitch, and the affine
//! transform from local map coordinates to output pixels.

use super::{MAX_PITCH, OBLIQUE_PITCH};
use crate::geo::{Point, Rect, TILE_SIZE};
use crate::map::Feature;
use crate::style::Ctx;

/// Simplification tolerance in pixels: vertices closer than this to the
/// previous kept vertex are dropped.
const TOLERANCE: f64 = 0.3;

/// A rectangle of the (possibly rotated and tilted) world at a given zoom, in
/// output pixels.
///
/// The camera is orthographic: the world is rotated so that compass direction
/// `bearing` points up the screen, then viewed from `pitch` degrees off
/// vertical, which foreshortens the ground by cos(pitch) and lifts anything
/// `h` meters tall by h·sin(pitch). That map is affine, so `x`/`y` live in a
/// flat "screen plane" and tiles are just a regular grid in it: a rotated,
/// tilted map is still tiled, cached and fetched in parallel like a normal one.
#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    /// Top-left corner in (rotated) world pixels at this zoom and scale.
    pub x: f64,
    pub y: f64,
    pub width: u32,
    pub height: u32,
    /// Style zoom (fractional zooms are fine).
    pub zoom: f64,
    /// Pixel ratio: 2 renders the same zoom at double resolution.
    pub scale: f32,
    /// Compass bearing at the top of the image, in degrees clockwise.
    pub bearing: f64,
    /// Camera angle from straight down, in degrees.
    pub pitch: f64,
    /// Whether the ground is foreshortened by the pitch (a true orthographic
    /// camera). Without it, only heights are tilted (an oblique projection
    /// that keeps the ground aligned with ordinary map tiles).
    pub foreshorten: bool,
}

impl Viewport {
    /// The XYZ tile `(z, x, y)`, with buildings in the default oblique view.
    pub fn tile(z: u8, x: u32, y: u32, scale: f32) -> Self {
        Viewport {
            foreshorten: false,
            ..Self::camera_tile(z, x as i64, y as i64, scale, 0.0, OBLIQUE_PITCH)
        }
    }

    /// Tile `(x, y)` of the grid in the screen plane of a camera with the
    /// given bearing and pitch. Indices may be negative once rotated.
    pub fn camera_tile(z: u8, x: i64, y: i64, scale: f32, bearing: f64, pitch: f64) -> Self {
        let size = (TILE_SIZE as f32 * scale).round() as u32;
        Viewport {
            x: x as f64 * size as f64,
            y: y as f64 * size as f64,
            width: size,
            height: size,
            zoom: z as f64,
            scale,
            bearing,
            pitch: pitch.clamp(0.0, MAX_PITCH),
            foreshorten: true,
        }
    }

    /// Vertical scale of the ground plane on screen.
    pub fn ground_scale(&self) -> f64 {
        if self.foreshorten {
            self.pitch.to_radians().cos()
        } else {
            1.0
        }
    }

    /// Screen pixels per pixel of height.
    pub fn lift(&self) -> f64 {
        self.pitch.to_radians().sin()
    }

    /// World width in output pixels.
    pub fn world_px(&self) -> f64 {
        TILE_SIZE * self.scale as f64 * self.zoom.exp2()
    }

    /// `(cos, sin)` of the bearing.
    pub fn rotation(&self) -> (f64, f64) {
        let b = self.bearing.to_radians();
        (b.cos(), b.sin())
    }

    /// World pixel (on the ground) → screen plane.
    pub fn forward(&self, [x, y]: Point) -> Point {
        let (c, s) = self.rotation();
        [c * x + s * y, (-s * x + c * y) * self.ground_scale()]
    }

    /// Screen plane → world pixel on the ground.
    pub fn inverse(&self, [x, y]: Point) -> Point {
        let (c, s) = self.rotation();
        let y = y / self.ground_scale();
        [c * x - s * y, s * x + c * y]
    }

    /// Normalized world bounds of the viewport grown by the given pixel
    /// margins (left/top/right and bottom).
    pub fn world_bounds(&self, margin: f64, below: f64) -> Rect {
        let k = self.world_px();
        let (x0, y0) = (self.x - margin, self.y - margin);
        let (x1, y1) = (
            self.x + self.width as f64 + margin,
            self.y + self.height as f64 + below,
        );
        let mut r = Rect::EMPTY;
        for p in [[x0, y0], [x1, y0], [x1, y1], [x0, y1]] {
            let [wx, wy] = self.inverse(p);
            r.extend([wx / k, wy / k]);
        }
        r
    }
}

/// Local → pixel transform (scale, rotate, foreshorten, translate) and the
/// per-viewport style context.
pub(super) struct Frame {
    /// Linear part: `x' = a·x + b·y`, `y' = c·x + d·y`.
    pub(super) a: f64,
    pub(super) b: f64,
    pub(super) c: f64,
    pub(super) d: f64,
    pub(super) bx: f64,
    pub(super) by: f64,
    /// Screen pixels of lift per meter of height.
    pub(super) lift_per_m: f64,
    /// World size in pixels (pixels per local unit).
    pub(super) k: f64,
    /// sin/cos of the bearing and of the pitch.
    pub(super) bearing_sc: (f64, f64),
    pub(super) pitch_sc: (f64, f64),
    pub(super) width: f64,
    pub(super) height: f64,
    pub(super) ctx: Ctx,
    pub(super) vis_zoom: f32,
}

impl Frame {
    pub(super) fn rect(&self, margin: f64) -> Rect {
        Rect::new(-margin, -margin, self.width + margin, self.height + margin)
    }

    /// Direction toward the viewer in [east, south, up] meters.
    pub(super) fn view_dir(&self) -> [f64; 3] {
        let ((bs, bc), (ps, pc)) = (self.bearing_sc, self.pitch_sc);
        [-bs * ps, bc * ps, pc]
    }

    /// How near a point (in [east, south, up] meters around any anchor) is
    /// to the viewer; larger is nearer.
    pub(super) fn closeness(&self, p: [f64; 3]) -> f64 {
        let v = self.view_dir();
        p[0] * v[0] + p[1] * v[1] + p[2] * v[2]
    }

    /// Screen position of a point given in meters [east, south, up] from an
    /// anchor whose ground position on screen is `at`.
    pub(super) fn meters_to_screen(&self, at: [f64; 2], p: [f64; 3]) -> [f64; 2] {
        let m = self.ctx.ppm as f64 / self.k;
        [
            at[0] + (self.a * p[0] + self.b * p[1]) * m,
            at[1] + (self.c * p[0] + self.d * p[1]) * m - p[2] * self.lift_per_m,
        ]
    }

    /// Meters per local map unit.
    pub(super) fn meters_per_unit(&self) -> f64 {
        self.k / self.ctx.ppm as f64
    }

    #[inline(always)]
    pub(super) fn apply(&self, x: f64, y: f64) -> Point {
        [
            self.a * x + self.b * y + self.bx,
            self.c * x + self.d * y + self.by,
        ]
    }

    /// Feature bounds in pixels (axis-aligned around the rotated box).
    pub(super) fn bbox(&self, f: &Feature) -> Rect {
        let b = f.bbox.map(|v| v as f64);
        if self.b == 0.0 && self.c == 0.0 {
            let (a, c) = (self.apply(b[0], b[1]), self.apply(b[2], b[3]));
            return Rect::new(a[0], a[1], c[0], c[1]);
        }
        let mut r = Rect::EMPTY;
        for (x, y) in [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])] {
            r.extend(self.apply(x, y));
        }
        r
    }

    /// Screen depth for back-to-front painting: the lowest screen point of
    /// the feature's box on the ground.
    pub(super) fn depth(&self, f: &Feature) -> f64 {
        self.box_depth(f.bbox)
    }

    /// Screen depth of a box's lowest ground point.
    pub(super) fn box_depth(&self, bbox: [f32; 4]) -> f64 {
        let b = bbox.map(|v| v as f64);
        [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])]
            .into_iter()
            .map(|(x, y)| self.c * x + self.d * y + self.by)
            .fold(f64::NEG_INFINITY, f64::max)
    }

    /// Projects a ring into `out`, dropping vertices within [`TOLERANCE`] of
    /// the previous one (the last vertex is always kept).
    pub(super) fn project(&self, ring: &[[f32; 2]], out: &mut Vec<Point>) {
        out.clear();
        let n = ring.len();
        for (i, p) in ring.iter().enumerate() {
            let q = self.apply(p[0] as f64, p[1] as f64);
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
