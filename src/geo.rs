//! Projection math and the clipping primitives shared by ingest and rendering.
//!
//! All geometry lives in *normalized Web Mercator* space: `x` and `y` both span
//! `[0, 1]` over the whole world, with `y` growing southward. This is the same
//! convention XYZ tiles use, so a tile `(z, x, y)` covers
//! `[x, x + 1] / 2^z` horizontally and `[y, y + 1] / 2^z` vertically.

use std::f64::consts::PI;

/// Latitude limit of Web Mercator (the square world).
pub const MAX_LAT: f64 = 85.051_128_779_806_59;
/// Equatorial circumference used by Web Mercator, in meters.
pub const EARTH_CIRCUMFERENCE: f64 = 40_075_016.685_578_49;
/// Edge length of a standard map tile in pixels.
pub const TILE_SIZE: f64 = 256.0;

pub type Point = [f64; 2];

/// Projects WGS84 longitude/latitude (degrees) to normalized Web Mercator.
pub fn project(lon: f64, lat: f64) -> Point {
    let x = (lon + 180.0) / 360.0;
    let lat = lat.clamp(-MAX_LAT, MAX_LAT).to_radians();
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / PI) / 2.0;
    [x, y]
}

/// Inverse of [`project`]: returns `(lon, lat)` in degrees.
pub fn unproject([x, y]: Point) -> (f64, f64) {
    let lon = x * 360.0 - 180.0;
    let lat = (PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees();
    (lon, lat)
}

/// Ground meters spanned by one normalized unit at Mercator row `y`.
pub fn meters_per_unit(y: f64) -> f64 {
    let (_, lat) = unproject([0.0, y]);
    EARTH_CIRCUMFERENCE * lat.to_radians().cos()
}

/// Axis-aligned rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl Rect {
    pub const EMPTY: Rect = Rect {
        min_x: f64::INFINITY,
        min_y: f64::INFINITY,
        max_x: f64::NEG_INFINITY,
        max_y: f64::NEG_INFINITY,
    };

    pub fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Self {
        Rect {
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }

    /// Mercator rectangle covering a lon/lat box.
    pub fn from_lon_lat(west: f64, south: f64, east: f64, north: f64) -> Self {
        let [min_x, min_y] = project(west, north);
        let [max_x, max_y] = project(east, south);
        Rect::new(min_x, min_y, max_x, max_y)
    }

    pub fn extend(&mut self, [x, y]: Point) {
        self.min_x = self.min_x.min(x);
        self.min_y = self.min_y.min(y);
        self.max_x = self.max_x.max(x);
        self.max_y = self.max_y.max(y);
    }

    pub fn union(&self, other: &Rect) -> Rect {
        Rect::new(
            self.min_x.min(other.min_x),
            self.min_y.min(other.min_y),
            self.max_x.max(other.max_x),
            self.max_y.max(other.max_y),
        )
    }

    pub fn is_empty(&self) -> bool {
        !(self.min_x <= self.max_x && self.min_y <= self.max_y)
    }

    pub fn width(&self) -> f64 {
        self.max_x - self.min_x
    }

    pub fn height(&self) -> f64 {
        self.max_y - self.min_y
    }

    pub fn center(&self) -> Point {
        [
            (self.min_x + self.max_x) / 2.0,
            (self.min_y + self.max_y) / 2.0,
        ]
    }

    pub fn expand(&self, by: f64) -> Rect {
        Rect::new(
            self.min_x - by,
            self.min_y - by,
            self.max_x + by,
            self.max_y + by,
        )
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        self.min_x <= other.max_x
            && other.min_x <= self.max_x
            && self.min_y <= other.max_y
            && other.min_y <= self.max_y
    }

    pub fn intersection(&self, other: &Rect) -> Rect {
        Rect::new(
            self.min_x.max(other.min_x),
            self.min_y.max(other.min_y),
            self.max_x.min(other.max_x),
            self.max_y.min(other.max_y),
        )
    }

    pub fn contains(&self, [x, y]: Point) -> bool {
        x >= self.min_x && x <= self.max_x && y >= self.min_y && y <= self.max_y
    }
}

/// Twice the signed area of a ring (shoelace). The ring may or may not repeat
/// its first point at the end.
pub fn signed_area2(ring: &[Point]) -> f64 {
    let n = ring.len();
    if n < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    let mut prev = ring[n - 1];
    for &p in ring {
        sum += (prev[0] - p[0]) * (prev[1] + p[1]);
        prev = p;
    }
    sum
}

/// Even-odd point-in-polygon test.
pub fn point_in_ring([x, y]: Point, ring: &[Point]) -> bool {
    let mut inside = false;
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let [xi, yi] = ring[i];
        let [xj, yj] = ring[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// A visible run of a clipped polyline: `points[start..start + len]`, beginning
/// `distance` units along the original line (used to keep dash phase continuous
/// across tile boundaries).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    pub start: usize,
    pub len: usize,
    pub distance: f64,
}

/// Liang–Barsky: the parametric interval of segment `a→b` inside `r`.
fn clip_segment(a: Point, b: Point, r: &Rect) -> Option<(f64, f64)> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let mut t0 = 0.0_f64;
    let mut t1 = 1.0_f64;
    for (p, q) in [
        (-d[0], a[0] - r.min_x),
        (d[0], r.max_x - a[0]),
        (-d[1], a[1] - r.min_y),
        (d[1], r.max_y - a[1]),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                if t > t1 {
                    return None;
                }
                t0 = t0.max(t);
            } else {
                if t < t0 {
                    return None;
                }
                t1 = t1.min(t);
            }
        }
    }
    Some((t0, t1))
}

fn lerp(a: Point, b: Point, t: f64) -> Point {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// Clips a polyline to `rect`, appending the visible points to `out` and one
/// [`Piece`] per contiguous visible run to `pieces`.
pub fn clip_polyline(line: &[Point], rect: &Rect, out: &mut Vec<Point>, pieces: &mut Vec<Piece>) {
    let mut open: Option<Piece> = None;
    let mut distance = 0.0;
    for w in line.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        match clip_segment(a, b, rect) {
            Some((t0, t1)) => {
                if open.is_none() || t0 > 0.0 {
                    if let Some(p) = open.take() {
                        pieces.push(p);
                    }
                    out.push(lerp(a, b, t0));
                    open = Some(Piece {
                        start: out.len() - 1,
                        len: 1,
                        distance: distance + t0 * len,
                    });
                }
                out.push(lerp(a, b, t1));
                if let Some(p) = open.as_mut() {
                    p.len += 1;
                }
                if t1 < 1.0 {
                    pieces.extend(open.take());
                }
            }
            None => pieces.extend(open.take()),
        }
        distance += len;
    }
    pieces.extend(open);
}

/// Sutherland–Hodgman: clips a closed ring to `rect` in place, using `scratch`
/// as a temporary buffer. Winding direction is preserved, so the result still
/// composes correctly under a non-zero fill rule.
pub fn clip_ring(ring: &mut Vec<Point>, rect: &Rect, scratch: &mut Vec<Point>) {
    // Each pass keeps the half-plane where `inside` holds.
    fn pass(
        src: &[Point],
        dst: &mut Vec<Point>,
        inside: impl Fn(Point) -> bool,
        cross: impl Fn(Point, Point) -> Point,
    ) {
        dst.clear();
        let Some(&last) = src.last() else { return };
        let mut prev = last;
        let mut prev_in = inside(prev);
        for &p in src {
            let p_in = inside(p);
            if p_in != prev_in {
                dst.push(cross(prev, p));
            }
            if p_in {
                dst.push(p);
            }
            prev = p;
            prev_in = p_in;
        }
    }
    let at_x = |x: f64| {
        move |a: Point, b: Point| {
            let t = (x - a[0]) / (b[0] - a[0]);
            [x, a[1] + (b[1] - a[1]) * t]
        }
    };
    let at_y = |y: f64| {
        move |a: Point, b: Point| {
            let t = (y - a[1]) / (b[1] - a[1]);
            [a[0] + (b[0] - a[0]) * t, y]
        }
    };
    pass(ring, scratch, |p| p[0] >= rect.min_x, at_x(rect.min_x));
    pass(scratch, ring, |p| p[0] <= rect.max_x, at_x(rect.max_x));
    pass(ring, scratch, |p| p[1] >= rect.min_y, at_y(rect.min_y));
    pass(scratch, ring, |p| p[1] <= rect.max_y, at_y(rect.max_y));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_round_trips() {
        for &(lon, lat) in &[
            (0.0, 0.0),
            (-73.98, 40.75),
            (139.69, 35.68),
            (-180.0, -85.0),
        ] {
            let (lon2, lat2) = unproject(project(lon, lat));
            assert!((lon - lon2).abs() < 1e-9 && (lat - lat2).abs() < 1e-9);
        }
        assert_eq!(project(0.0, 0.0), [0.5, 0.5]);
        // North is up: higher latitudes have smaller y.
        assert!(project(0.0, 60.0)[1] < project(0.0, 10.0)[1]);
    }

    #[test]
    fn meters_per_unit_shrinks_with_latitude() {
        let equator = meters_per_unit(0.5);
        assert!((equator - EARTH_CIRCUMFERENCE).abs() < 1e-3);
        let nyc = meters_per_unit(project(0.0, 40.7)[1]);
        assert!((nyc / equator - 40.7_f64.to_radians().cos()).abs() < 1e-9);
    }

    #[test]
    fn polyline_clipping_splits_into_pieces() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        // In, out, back in.
        let line = [[5.0, 5.0], [15.0, 5.0], [15.0, 8.0], [5.0, 8.0]];
        let (mut pts, mut pieces) = (vec![], vec![]);
        clip_polyline(&line, &r, &mut pts, &mut pieces);
        assert_eq!(pieces.len(), 2);
        assert_eq!(&pts[..2], &[[5.0, 5.0], [10.0, 5.0]]);
        assert_eq!(&pts[2..], &[[10.0, 8.0], [5.0, 8.0]]);
        // Second piece starts after 10 + 3 + 5 units of travel.
        assert!((pieces[1].distance - 18.0).abs() < 1e-9);
    }

    #[test]
    fn polyline_fully_outside_is_dropped() {
        let r = Rect::new(0.0, 0.0, 1.0, 1.0);
        let (mut pts, mut pieces) = (vec![], vec![]);
        clip_polyline(&[[2.0, 2.0], [3.0, 3.0]], &r, &mut pts, &mut pieces);
        assert!(pieces.is_empty() && pts.is_empty());
    }

    #[test]
    fn ring_clipping_keeps_orientation() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        let mut ring = vec![[-5.0, -5.0], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]];
        let before = signed_area2(&ring).signum();
        clip_ring(&mut ring, &r, &mut Vec::new());
        assert_eq!(signed_area2(&ring).signum(), before);
        assert!((signed_area2(&ring).abs() / 2.0 - 25.0).abs() < 1e-9);
    }

    #[test]
    fn point_in_ring_works() {
        let sq = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]];
        assert!(point_in_ring([2.0, 2.0], &sq));
        assert!(!point_in_ring([5.0, 2.0], &sq));
    }
}
