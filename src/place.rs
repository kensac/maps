//! Street furniture placement: puts roadside objects where they actually
//! stand and turns them the way they face.
//!
//! OSM maps traffic controls on the road's centerline node and most other
//! furniture as a free node beside the road. Drawn in 3D, a stop sign in the
//! middle of the road or a bench facing a wall looks wrong, so:
//!
//! - **Stop/yield signs and mid-block signals** move to the right-hand curb of
//!   the traffic they control (`direction=forward/backward`, else inferred:
//!   the end of the road nearest the node is the junction being approached)
//!   and face that traffic.
//! - **Signals on an intersection node** become one pole per approach, at the
//!   right-hand corner, facing it.
//! - **Lamps and other objects on a road** move to its curb and face it;
//!   power poles and pylons turn their crossarms across their line.
//! - **Objects beside a road** (benches, shelters, lamps, hydrants...) face the
//!   nearest road within reach; subway stairs run along it.
//!
//! Traffic is assumed to keep right. Headings are compass radians: the
//! direction an object's front (its sign face, lamp arm, open side) faces.

use crate::classify::{Facing, Kind};
use crate::geo::{meters_per_unit, Point};
use crate::ingest::{RawData, RawPoint};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

/// How far from an object to look for the road it belongs to, in meters.
const REACH: f64 = 25.0;
/// Gap between the edge of the carriageway and a curbside object, in meters.
const CURB: f64 = 1.0;

/// A point object with its final position and heading (`NaN`: unturned).
pub struct Placed {
    pub point: RawPoint,
    pub heading: f32,
}

fn needs_placing(kind: Kind) -> bool {
    use Kind::*;
    matches!(
        kind,
        StopSign
            | TrafficSignal
            | StreetLamp
            | SubwayEntrance
            | Bench
            | Shelter
            | PostBox
            | Hydrant
            | WasteBasket
            | Phone
            | BicycleParking
            | Cabinet
            | RailSignal
            | PowerPole
            | PowerTower
            | BufferStop
    )
}

/// Half the width of the carriageway (or track bed) of a line kind.
fn half_width(kind: Kind) -> f64 {
    use Kind::*;
    match kind {
        Motorway => 7.5,
        Trunk => 7.0,
        Primary => 6.5,
        Secondary => 6.0,
        Tertiary => 5.5,
        Minor | PedestrianStreet => 4.5,
        Service => 3.0,
        Track => 2.0,
        Footway | Cycleway => 1.2,
        Rail | Subway | LightRail | Tram => 2.0,
        _ => 0.5,
    }
}

/// The family of lines an object belongs beside.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    Road,
    Rail,
    Power,
}

fn family_of_object(kind: Kind) -> Family {
    match kind {
        Kind::RailSignal | Kind::BufferStop => Family::Rail,
        Kind::PowerPole | Kind::PowerTower => Family::Power,
        _ => Family::Road,
    }
}

fn family_of_line(kind: Kind) -> Option<Family> {
    match kind {
        Kind::Rail | Kind::Subway | Kind::LightRail | Kind::Tram => Some(Family::Rail),
        k if k.is_transport() => Some(Family::Road),
        Kind::PowerLine => Some(Family::Power),
        _ => None,
    }
}

fn norm(v: Point) -> Point {
    let l = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if l == 0.0 {
        [0.0, 0.0]
    } else {
        [v[0] / l, v[1] / l]
    }
}

/// Compass heading of a direction in (east, south) coordinates.
fn heading(v: Point) -> f32 {
    v[0].atan2(-v[1]) as f32
}

/// Visual right of travel direction `d` (east, south).
fn right(d: Point) -> Point {
    [-d[1], d[0]]
}

fn add(p: Point, v: Point, meters: f64, mpu: f64) -> Point {
    [p[0] + v[0] * meters / mpu, p[1] + v[1] * meters / mpu]
}

/// A segment of a nearby line: its family, kind and endpoints.
type Segment = (Family, Kind, Point, Point);
/// Nearby line segments, bucketed by grid cell.
type Grid = FxHashMap<(i64, i64), Vec<Segment>>;

/// A line passing through an object's node.
struct OnLine {
    kind: Kind,
    tangent: Point,
    /// Whether the node is nearer the line's end than its start.
    near_end: bool,
    /// Directions from the node to its neighbours along this line.
    out: Vec<Point>,
}

pub fn place(raw: &RawData) -> Vec<Placed> {
    let wanted: FxHashSet<i64> = raw
        .points
        .iter()
        .filter(|p| needs_placing(p.kind))
        .map(|p| p.id)
        .collect();

    // Lines through each object's node.
    let on_lines: FxHashMap<i64, Vec<OnLine>> = raw
        .ways
        .par_iter()
        .filter(|w| {
            family_of_line(w.kind).is_some() && w.flags & crate::classify::flags::TUNNEL == 0
        })
        .flat_map_iter(|w| {
            let n = w.refs.len();
            w.refs
                .iter()
                .enumerate()
                .filter(|(_, id)| wanted.contains(id))
                .filter_map(move |(i, &id)| {
                    let at = raw.node(id)?;
                    let prev = (i > 0).then(|| raw.node(w.refs[i - 1])).flatten();
                    let next = (i + 1 < n).then(|| raw.node(w.refs[i + 1])).flatten();
                    let tangent = norm(match (prev, next) {
                        (Some(a), Some(b)) => [b[0] - a[0], b[1] - a[1]],
                        (Some(a), None) => [at[0] - a[0], at[1] - a[1]],
                        (None, Some(b)) => [b[0] - at[0], b[1] - at[1]],
                        (None, None) => return None,
                    });
                    let out = [prev, next]
                        .into_iter()
                        .flatten()
                        .map(|q| norm([q[0] - at[0], q[1] - at[1]]))
                        .collect();
                    Some((
                        id,
                        OnLine {
                            kind: w.kind,
                            tangent,
                            near_end: i * 2 >= n.saturating_sub(1),
                            out,
                        },
                    ))
                })
        })
        .fold(
            FxHashMap::default,
            |mut m: FxHashMap<i64, Vec<OnLine>>, (id, l)| {
                m.entry(id).or_default().push(l);
                m
            },
        )
        .reduce(FxHashMap::default, |mut a, b| {
            for (k, v) in b {
                a.entry(k).or_default().extend(v);
            }
            a
        });

    // Road segments near the remaining objects, bucketed by grid cell.
    let cell_size = REACH / meters_per_unit(raw.node_bbox.center()[1]);
    let cell = |p: Point| {
        (
            (p[0] / cell_size).floor() as i64,
            (p[1] / cell_size).floor() as i64,
        )
    };
    let free: Vec<&RawPoint> = raw
        .points
        .iter()
        .filter(|p| needs_placing(p.kind) && !on_lines.contains_key(&p.id))
        .collect();
    let cells: FxHashSet<(i64, i64)> = free
        .iter()
        .flat_map(|p| {
            let (cx, cy) = cell(p.at);
            (-1..=1).flat_map(move |dx| (-1..=1).map(move |dy| (cx + dx, cy + dy)))
        })
        .collect();
    let grid: Grid = raw
        .ways
        .par_iter()
        .filter_map(|w| Some((family_of_line(w.kind)?, w)))
        .filter(|(_, w)| w.flags & crate::classify::flags::TUNNEL == 0)
        .flat_map_iter(|(fam, w)| {
            let cells = &cells;
            w.refs.windows(2).filter_map(move |pair| {
                let (a, b) = (raw.node(pair[0])?, raw.node(pair[1])?);
                let (ca, cb) = (cell(a), cell(b));
                let mut hit = Vec::new();
                for x in ca.0.min(cb.0)..=ca.0.max(cb.0) {
                    for y in ca.1.min(cb.1)..=ca.1.max(cb.1) {
                        if cells.contains(&(x, y)) {
                            hit.push(((x, y), (fam, w.kind, a, b)));
                        }
                    }
                }
                (!hit.is_empty()).then_some(hit)
            })
        })
        .flatten_iter()
        .fold(
            FxHashMap::default,
            |mut m: FxHashMap<_, Vec<_>>, (c, seg)| {
                m.entry(c).or_default().push(seg);
                m
            },
        )
        .reduce(FxHashMap::default, |mut a, b| {
            for (k, v) in b {
                a.entry(k).or_default().extend(v);
            }
            a
        });

    raw.points
        .par_iter()
        .flat_map_iter(|p| {
            let mut out = Vec::new();
            place_one(p, &on_lines, &grid, cell, &mut out);
            out
        })
        .collect()
}

fn place_one(
    p: &RawPoint,
    on_lines: &FxHashMap<i64, Vec<OnLine>>,
    grid: &Grid,
    cell: impl Fn(Point) -> (i64, i64),
    out: &mut Vec<Placed>,
) {
    let unturned = |out: &mut Vec<Placed>| {
        out.push(Placed {
            point: *p,
            heading: f32::NAN,
        })
    };
    if !needs_placing(p.kind) {
        return unturned(out);
    }
    let mpu = meters_per_unit(p.at[1]);
    let family = family_of_object(p.kind);
    let explicit = match p.facing {
        Facing::Bearing(b) => Some(b.to_radians()),
        _ => None,
    };

    let lines: Vec<&OnLine> = on_lines
        .get(&p.id)
        .map(|v| {
            v.iter()
                .filter(|l| family_of_line(l.kind) == Some(family))
                .collect()
        })
        .unwrap_or_default();
    if let Some(line) = lines.first() {
        let hw = half_width(line.kind);
        match p.kind {
            Kind::PowerPole | Kind::PowerTower => out.push(Placed {
                point: *p,
                heading: heading(line.tangent),
            }),
            Kind::TrafficSignal if lines.iter().map(|l| l.out.len()).sum::<usize>() >= 3 => {
                // An intersection: a pole at the right-hand corner of each
                // approach, facing it.
                let cross = lines.iter().map(|l| half_width(l.kind)).fold(0.0, f64::max);
                for l in &lines {
                    if matches!(l.kind, Kind::Footway | Kind::Cycleway) {
                        continue;
                    }
                    for &u in &l.out {
                        let travel = [-u[0], -u[1]];
                        let at = add(
                            add(p.at, u, cross + CURB, mpu),
                            right(travel),
                            half_width(l.kind) + CURB,
                            mpu,
                        );
                        out.push(Placed {
                            point: RawPoint { at, ..*p },
                            heading: heading(u),
                        });
                    }
                }
            }
            Kind::StopSign | Kind::TrafficSignal | Kind::RailSignal => {
                // Beside the traffic it controls, facing it.
                let t = line.tangent;
                let travel = match p.facing {
                    Facing::Forward => t,
                    Facing::Backward => [-t[0], -t[1]],
                    _ if line.near_end => t,
                    _ => [-t[0], -t[1]],
                };
                let at = add(p.at, right(travel), hw + CURB, mpu);
                out.push(Placed {
                    point: RawPoint { at, ..*p },
                    heading: explicit.unwrap_or(heading([-travel[0], -travel[1]])),
                });
            }
            _ => {
                // To the curb, facing the road.
                let side = right(line.tangent);
                let at = add(p.at, side, hw + CURB, mpu);
                out.push(Placed {
                    point: RawPoint { at, ..*p },
                    heading: explicit.unwrap_or(heading([-side[0], -side[1]])),
                });
            }
        }
        return;
    }

    if let Some(h) = explicit {
        out.push(Placed {
            point: *p,
            heading: h,
        });
        return;
    }
    // Beside a road: face the nearest one within reach.
    let (cx, cy) = cell(p.at);
    let mut best: Option<(f64, Point, Point)> = None;
    for dx in -1..=1 {
        for dy in -1..=1 {
            for &(fam, _, a, b) in grid.get(&(cx + dx, cy + dy)).into_iter().flatten() {
                if fam != family {
                    continue;
                }
                let ab = [b[0] - a[0], b[1] - a[1]];
                let len2 = ab[0] * ab[0] + ab[1] * ab[1];
                let t = if len2 > 0.0 {
                    (((p.at[0] - a[0]) * ab[0] + (p.at[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let q = [a[0] + ab[0] * t, a[1] + ab[1] * t];
                let d = ((q[0] - p.at[0]).powi(2) + (q[1] - p.at[1]).powi(2)).sqrt() * mpu;
                if d <= REACH && best.is_none_or(|(bd, ..)| d < bd) {
                    best = Some((d, q, norm(ab)));
                }
            }
        }
    }
    match best {
        Some((d, q, along)) if d > 0.01 => {
            let toward = norm([q[0] - p.at[0], q[1] - p.at[1]]);
            let h = if p.kind == Kind::SubwayEntrance {
                heading(along)
            } else {
                heading(toward)
            };
            out.push(Placed {
                point: *p,
                heading: h,
            });
        }
        _ => unturned(out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_are_compass_bearings() {
        let deg = |v: Point| heading(v).to_degrees().rem_euclid(360.0).round();
        assert_eq!(deg([0.0, -1.0]), 0.0); // north
        assert_eq!(deg([1.0, 0.0]), 90.0); // east
        assert_eq!(deg([0.0, 1.0]), 180.0); // south
        assert_eq!(deg([-1.0, 0.0]), 270.0); // west
    }

    #[test]
    fn right_of_travel() {
        // Driving north, the right-hand curb is to the east.
        assert_eq!(right([0.0, -1.0]), [1.0, 0.0]);
        // Driving east, it is to the south.
        assert_eq!(right([1.0, 0.0]), [0.0, 1.0]);
    }
}
