//! Geometry assembly: stitching ways into rings, orienting rings so a single
//! non-zero fill renders holes correctly, and turning coastlines into land.

use crate::geo::{clip_polyline, point_in_ring, signed_area2, Piece, Point, Rect};
use rustc_hash::FxHashMap;

/// Joins way node-ID sequences into rings by matching shared endpoints,
/// reversing ways as needed (multipolygon members have no required direction).
/// Rings that cannot be closed are returned open; a fill closes them implicitly.
pub fn assemble_rings(ways: &[&[i64]]) -> Vec<Vec<i64>> {
    let mut rings = Vec::new();
    let mut by_endpoint: FxHashMap<i64, Vec<usize>> = FxHashMap::default();
    let mut used = vec![false; ways.len()];
    for (i, w) in ways.iter().enumerate() {
        if w.len() < 2 {
            used[i] = true;
        } else if w.first() == w.last() {
            used[i] = true;
            rings.push(w.to_vec());
        } else {
            by_endpoint.entry(w[0]).or_default().push(i);
            by_endpoint.entry(w[w.len() - 1]).or_default().push(i);
        }
    }

    for start in 0..ways.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut ring = ways[start].to_vec();
        let mut flipped = false;
        while ring.first() != ring.last() {
            let end = *ring.last().unwrap();
            let next = by_endpoint
                .get(&end)
                .and_then(|c| c.iter().copied().find(|&j| !used[j]));
            match next {
                Some(j) => {
                    used[j] = true;
                    let w = ways[j];
                    if w[0] == end {
                        ring.extend_from_slice(&w[1..]);
                    } else {
                        ring.extend(w.iter().rev().skip(1));
                    }
                }
                // Dead end: try growing from the other end once.
                None if !flipped => {
                    flipped = true;
                    ring.reverse();
                }
                None => break,
            }
        }
        if ring.len() >= 3 {
            rings.push(ring);
        }
    }
    rings
}

/// Orients the rings of one polygon so that rings at even nesting depth
/// (outers) have positive area and rings at odd depth (holes) negative.
/// Afterwards a non-zero fill renders it correctly, and so does any union of
/// such polygons drawn as a single path.
pub fn orient_rings(rings: &mut [Vec<Point>]) {
    if rings.len() == 1 {
        if signed_area2(&rings[0]) < 0.0 {
            rings[0].reverse();
        }
        return;
    }
    let bboxes: Vec<Rect> = rings
        .iter()
        .map(|r| {
            let mut b = Rect::EMPTY;
            r.iter().for_each(|&p| b.extend(p));
            b
        })
        .collect();
    let depths: Vec<usize> = (0..rings.len())
        .map(|i| {
            let Some(&probe) = rings[i].first() else {
                return 0;
            };
            (0..rings.len())
                .filter(|&j| {
                    j != i
                        && bboxes[j].contains(probe)
                        && bboxes[j].width() * bboxes[j].height()
                            >= bboxes[i].width() * bboxes[i].height()
                        && point_in_ring(probe, &rings[j])
                })
                .count()
        })
        .collect();
    for (ring, depth) in rings.iter_mut().zip(depths) {
        let positive = signed_area2(ring) > 0.0;
        if positive != (depth % 2 == 0) {
            ring.reverse();
        }
    }
}

/// Joins coastline ways end-to-start into chains. Coastlines are directed
/// (land on the left), so ways are never reversed.
pub fn assemble_coastlines(ways: Vec<Vec<i64>>) -> Vec<Vec<i64>> {
    let mut by_start: FxHashMap<i64, usize> = FxHashMap::default();
    let mut ends: rustc_hash::FxHashSet<i64> = Default::default();
    for (i, w) in ways.iter().enumerate() {
        by_start.insert(w[0], i);
        ends.insert(*w.last().unwrap());
    }
    let mut used = vec![false; ways.len()];
    let mut chains = Vec::new();
    // Chain heads (nothing flows into them) first, then whatever is left: loops.
    let heads = (0..ways.len()).filter(|&i| !ends.contains(&ways[i][0]));
    let order: Vec<usize> = heads.chain(0..ways.len()).collect();
    for start in order {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut chain = ways[start].clone();
        while chain.first() != chain.last() {
            match by_start.get(chain.last().unwrap()) {
                Some(&j) if !used[j] => {
                    used[j] = true;
                    chain.extend_from_slice(&ways[j][1..]);
                }
                _ => break,
            }
        }
        chains.push(chain);
    }
    chains
}

/// Whether the area outside all land polygons is land or sea.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Background {
    Land,
    Water,
}

/// Position along the boundary of `r`, walking counter-clockwise *as seen on
/// the map* (y grows downward): top edge right→left, left edge down, bottom
/// edge left→right, right edge up. Returns the snapped point and `t ∈ [0, 4)`.
fn perimeter_position(r: &Rect, [x, y]: Point) -> (Point, f64) {
    let (w, h) = (r.width(), r.height());
    let x = x.clamp(r.min_x, r.max_x);
    let y = y.clamp(r.min_y, r.max_y);
    let d_top = y - r.min_y;
    let d_left = x - r.min_x;
    let d_bottom = r.max_y - y;
    let d_right = r.max_x - x;
    let m = d_top.min(d_left).min(d_bottom).min(d_right);
    if m == d_top {
        ([x, r.min_y], (r.max_x - x) / w)
    } else if m == d_left {
        ([r.min_x, y], 1.0 + (y - r.min_y) / h)
    } else if m == d_bottom {
        ([x, r.max_y], 2.0 + (x - r.min_x) / w)
    } else {
        ([r.max_x, y], (3.0 + (r.max_y - y) / h) % 4.0)
    }
}

fn corner(r: &Rect, k: i64) -> Point {
    match k.rem_euclid(4) {
        0 => [r.max_x, r.min_y],
        1 => [r.min_x, r.min_y],
        2 => [r.min_x, r.max_y],
        _ => [r.max_x, r.max_y],
    }
}

/// Builds land polygons from coastline chains inside `rect`.
///
/// Closed chains are islands. Open chains are clipped to `rect`; each visible
/// piece enters and leaves through the boundary, and since land lies to the
/// left of a coastline, walking counter-clockwise along the boundary from a
/// piece's exit reaches the entry of the next piece of the same land polygon.
/// All rings come out consistently oriented, so a non-zero fill of their union
/// gives the land (inner seas wind the other way and cancel out).
pub fn land_polygons(chains: &[Vec<Point>], rect: &Rect) -> (Background, Vec<Vec<Point>>) {
    if chains.is_empty() {
        return (Background::Land, Vec::new());
    }
    let mut rings = Vec::new();
    struct Open {
        points: Vec<Point>,
        t_in: f64,
        t_out: f64,
    }
    let mut pieces: Vec<Open> = Vec::new();
    let (mut buf, mut runs) = (Vec::new(), Vec::new());

    for chain in chains {
        if chain.len() >= 4 && chain.first() == chain.last() {
            rings.push(chain.clone());
            continue;
        }
        buf.clear();
        runs.clear();
        clip_polyline(chain, rect, &mut buf, &mut runs);
        for &Piece { start, len, .. } in &runs {
            if len < 2 {
                continue;
            }
            let pts = &buf[start..start + len];
            // Extracts cut at the box can leave chain ends inside it; extend
            // them to the nearest edge so the walk below still closes.
            let (p_in, t_in) = perimeter_position(rect, pts[0]);
            let (p_out, t_out) = perimeter_position(rect, pts[len - 1]);
            let mut points = Vec::with_capacity(len + 2);
            if p_in != pts[0] {
                points.push(p_in);
            }
            points.extend_from_slice(pts);
            if p_out != pts[len - 1] {
                points.push(p_out);
            }
            pieces.push(Open {
                points,
                t_in,
                t_out,
            });
        }
    }

    if pieces.is_empty() {
        // Only islands (or nothing) inside the box: it is all sea around them.
        return (Background::Water, rings);
    }

    let mut used = vec![false; pieces.len()];
    for first in 0..pieces.len() {
        if used[first] {
            continue;
        }
        let mut ring: Vec<Point> = Vec::new();
        let mut cur = first;
        loop {
            used[cur] = true;
            ring.extend_from_slice(&pieces[cur].points);
            let t_out = pieces[cur].t_out;
            let (next, gap) = pieces
                .iter()
                .enumerate()
                .filter(|&(j, _)| !used[j] || j == first)
                .map(|(j, p)| (j, (p.t_in - t_out).rem_euclid(4.0)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .expect("at least the first piece is a candidate");
            let mut k = t_out.floor() as i64 + 1;
            while (k as f64) < t_out + gap {
                ring.push(corner(rect, k));
                k += 1;
            }
            if next == first {
                break;
            }
            cur = next;
        }
        rings.push(ring);
    }
    (Background::Water, rings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rings_assemble_from_reversed_parts() {
        let a = [1, 2, 3];
        let b = [5, 4, 3]; // reversed relative to a
        let c = [5, 6, 1];
        let rings = assemble_rings(&[&a, &b, &c]);
        assert_eq!(rings.len(), 1);
        let r = &rings[0];
        assert_eq!(r.first(), r.last());
        assert_eq!(r.len(), 7);
    }

    #[test]
    fn closed_ways_pass_through() {
        let a = [1, 2, 3, 1];
        assert_eq!(assemble_rings(&[&a]), vec![vec![1, 2, 3, 1]]);
    }

    #[test]
    fn holes_get_opposite_orientation() {
        let outer = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let hole = vec![[2.0, 2.0], [8.0, 2.0], [8.0, 8.0], [2.0, 8.0]];
        let island = vec![[4.0, 4.0], [6.0, 4.0], [6.0, 6.0], [4.0, 6.0]];
        let mut rings = vec![hole, outer, island];
        orient_rings(&mut rings);
        assert!(signed_area2(&rings[0]) < 0.0, "hole");
        assert!(signed_area2(&rings[1]) > 0.0, "outer");
        assert!(signed_area2(&rings[2]) > 0.0, "island in hole");
    }

    #[test]
    fn coastline_chains_join_in_direction() {
        let chains = assemble_coastlines(vec![vec![3, 4], vec![1, 2, 3], vec![9, 8]]);
        assert_eq!(chains, vec![vec![1, 2, 3, 4], vec![9, 8]]);
    }

    fn area(rings: &[Vec<Point>]) -> f64 {
        rings
            .iter()
            .map(|r| signed_area2(r) / 2.0)
            .sum::<f64>()
            .abs()
    }

    #[test]
    fn straight_coast_makes_west_land() {
        // Coast runs north (y decreasing) through x = 4: land is to the west.
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        let coast = vec![[4.0, 12.0], [4.0, -2.0]];
        let (bg, rings) = land_polygons(&[coast], &r);
        assert_eq!(bg, Background::Water);
        assert_eq!(rings.len(), 1);
        assert!((area(&rings) - 40.0).abs() < 1e-9);
        assert!(point_in_ring([1.0, 5.0], &rings[0]));
        assert!(!point_in_ring([8.0, 5.0], &rings[0]));
    }

    #[test]
    fn coast_flowing_south_makes_east_land() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        let coast = vec![[4.0, -2.0], [4.0, 12.0]];
        let (_, rings) = land_polygons(&[coast], &r);
        assert!((area(&rings) - 60.0).abs() < 1e-9);
        assert!(point_in_ring([8.0, 5.0], &rings[0]));
    }

    #[test]
    fn bay_wraps_around_corners() {
        // A U-shaped coast entering and leaving through the top: the sea is
        // the inside of the U (on the right of travel), land everywhere else.
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        let coast = vec![[7.0, -1.0], [7.0, 5.0], [3.0, 5.0], [3.0, -1.0]];
        let (_, rings) = land_polygons(&[coast], &r);
        assert_eq!(rings.len(), 1);
        assert!((area(&rings) - (100.0 - 20.0)).abs() < 1e-9);
        assert!(!point_in_ring([5.0, 2.0], &rings[0]));
        assert!(point_in_ring([5.0, 8.0], &rings[0]));
    }

    #[test]
    fn two_coasts_form_separate_land_masses() {
        // A north-flowing coast at x = 3 (land west) and a south-flowing one
        // at x = 7 (land east): a channel of sea in between.
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        let west = vec![[3.0, 11.0], [3.0, -1.0]];
        let east = vec![[7.0, -1.0], [7.0, 11.0]];
        let (_, rings) = land_polygons(&[west, east], &r);
        assert_eq!(rings.len(), 2);
        assert!((area(&rings) - 60.0).abs() < 1e-9);
    }

    #[test]
    fn islands_only_means_sea_background() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        let island = vec![[4.0, 4.0], [4.0, 6.0], [6.0, 6.0], [6.0, 4.0], [4.0, 4.0]];
        let (bg, rings) = land_polygons(&[island], &r);
        assert_eq!(bg, Background::Water);
        assert_eq!(rings.len(), 1);
    }

    #[test]
    fn broken_chain_end_is_snapped_to_boundary() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        // Starts outside the bottom, ends inside near the top edge.
        let coast = vec![[4.0, 12.0], [4.0, 1.0]];
        let (_, rings) = land_polygons(&[coast], &r);
        assert!((area(&rings) - 40.0).abs() < 1e-9);
    }
}
