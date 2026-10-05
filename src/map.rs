//! The render-ready map: flat geometry arenas, features pre-sorted into draw
//! order, and a spatial index bucketed by the zoom at which features appear.

use crate::assemble::{
    assemble_coastlines, assemble_rings, land_polygons, orient_rings, Background,
};
use crate::classify::{Group, Kind};
use crate::geo::{signed_area2, Point, Rect, TILE_SIZE};
use crate::ingest::RawData;
use rayon::prelude::*;
use rstar::primitives::{GeomWithData, Rectangle};
use rstar::{RTree, AABB};
use std::time::Instant;

/// Highest zoom bucket in the index; anything visible later shares it.
const MAX_BUCKET: usize = 20;

#[derive(Clone, Copy, Debug)]
pub struct Feature {
    pub kind: Kind,
    pub flags: u8,
    pub layer: i8,
    /// Index of the first ring in [`Map::ring_starts`].
    pub ring_start: u32,
    pub ring_count: u32,
    /// Building height in meters; admin level for boundaries.
    pub height: f32,
    /// Zoom from which the feature is drawn: its kind's minimum zoom, or the
    /// zoom at which it grows past a pixel, whichever is later.
    pub vis_zoom: f32,
    /// `[min_x, min_y, max_x, max_y]` in local coordinates.
    pub bbox: [f32; 4],
}

impl Feature {
    pub fn group(&self) -> Group {
        self.kind.group(self.layer, self.flags)
    }
}

type Entry = GeomWithData<Rectangle<[f32; 2]>, u32>;

pub struct Map {
    /// Data extent in normalized Mercator coordinates.
    pub bounds: Rect,
    /// Local coordinates are `world - origin`, stored as `f32`.
    pub origin: Point,
    pub background: Background,
    pub points: Vec<[f32; 2]>,
    /// Ring `r` spans `points[ring_starts[r]..ring_starts[r + 1]]`.
    pub ring_starts: Vec<u32>,
    /// Sorted in draw order.
    pub features: Vec<Feature>,
    index: Vec<RTree<Entry>>,
}

/// Accumulates features for one chunk of input; chunks build in parallel and
/// are concatenated afterwards.
#[derive(Default)]
struct Chunk {
    points: Vec<[f32; 2]>,
    ring_starts: Vec<u32>,
    features: Vec<(Feature, f32)>,
}

impl Chunk {
    fn push(
        &mut self,
        origin: Point,
        kind: Kind,
        flags: u8,
        layer: i8,
        height: f32,
        rings: &[Vec<Point>],
    ) {
        let mut bbox = Rect::EMPTY;
        let ring_start = self.ring_starts.len() as u32;
        let mut area2 = 0.0;
        for ring in rings {
            if ring.is_empty() {
                continue;
            }
            self.ring_starts.push(self.points.len() as u32);
            for &p in ring {
                bbox.extend(p);
                self.points
                    .push([(p[0] - origin[0]) as f32, (p[1] - origin[1]) as f32]);
            }
            if kind.is_area() {
                area2 += signed_area2(ring);
            }
        }
        let ring_count = self.ring_starts.len() as u32 - ring_start;
        if ring_count == 0 {
            return;
        }
        let extent = bbox.width().max(bbox.height());
        let min_px = if kind.is_area() { 1.0 } else { 1.5 };
        let size_zoom = if kind.is_point() || kind == Kind::Land {
            0.0
        } else if extent > 0.0 {
            (min_px / (extent * TILE_SIZE)).log2() as f32
        } else {
            f32::INFINITY
        };
        let mut vis_zoom = kind.min_zoom(flags).max(size_zoom);
        if kind == Kind::Boundary {
            // Height carries the admin level: local borders only up close.
            vis_zoom = vis_zoom.max(match height as u8 {
                0..=4 => 4.0,
                5..=6 => 9.0,
                _ => 12.0,
            });
        }
        if !vis_zoom.is_finite() {
            self.ring_starts.truncate(ring_start as usize);
            return;
        }
        let local = |v: f64, o: f64| (v - o) as f32;
        self.features.push((
            Feature {
                kind,
                flags,
                layer,
                ring_start,
                ring_count,
                height,
                vis_zoom,
                bbox: [
                    local(bbox.min_x, origin[0]),
                    local(bbox.min_y, origin[1]),
                    local(bbox.max_x, origin[0]),
                    local(bbox.max_y, origin[1]),
                ],
            },
            (area2.abs() / 2.0) as f32,
        ));
    }
}

/// Resolves node IDs to coordinates, dropping missing nodes and repeats.
fn resolve(raw: &RawData, refs: &[i64]) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(refs.len());
    for &id in refs {
        if let Some(p) = raw.node(id) {
            if out.last() != Some(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// Polygon rings are stored without the closing repeat of the first point.
fn open_ring(mut ring: Vec<Point>) -> Vec<Point> {
    if ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }
    ring
}

impl Map {
    pub fn build(raw: RawData) -> Map {
        let t = Instant::now();
        let bounds = raw
            .header_bbox
            .filter(|b| !b.is_empty())
            .unwrap_or(raw.node_bbox);
        let origin = [bounds.min_x, bounds.min_y];

        let ways = raw.ways.par_chunks(4096).map(|chunk| {
            let mut c = Chunk::default();
            for w in chunk {
                let pts = resolve(&raw, &w.refs);
                if w.kind.is_area() {
                    let mut rings = [open_ring(pts)];
                    if rings[0].len() < 3 {
                        continue;
                    }
                    orient_rings(&mut rings);
                    c.push(origin, w.kind, w.flags, w.layer, w.height, &rings);
                } else if pts.len() >= 2 {
                    c.push(origin, w.kind, w.flags, w.layer, w.height, &[pts]);
                }
            }
            c
        });

        let multipolygons = raw.multipolygons.par_chunks(256).map(|chunk| {
            let mut c = Chunk::default();
            for mp in chunk {
                let members: Vec<&[i64]> = mp
                    .members
                    .iter()
                    .filter_map(|id| raw.member_ways.get(id).map(Vec::as_slice))
                    .collect();
                let mut rings: Vec<Vec<Point>> = assemble_rings(&members)
                    .iter()
                    .map(|r| open_ring(resolve(&raw, r)))
                    .filter(|r| r.len() >= 3)
                    .collect();
                if rings.is_empty() {
                    continue;
                }
                orient_rings(&mut rings);
                c.push(origin, mp.kind, mp.flags, mp.layer, mp.height, &rings);
            }
            c
        });

        let boundaries =
            raw.boundary_ways
                .par_iter()
                .fold(Chunk::default, |mut c, (id, &level)| {
                    if let Some(refs) = raw.member_ways.get(id) {
                        let pts = resolve(&raw, refs);
                        if pts.len() >= 2 {
                            c.push(origin, Kind::Boundary, 0, 0, level as f32, &[pts]);
                        }
                    }
                    c
                });

        let points = raw.points.par_chunks(65536).map(|chunk| {
            let mut c = Chunk::default();
            for &(kind, p) in chunk {
                c.push(origin, kind, 0, 0, 0.0, &[vec![p]]);
            }
            c
        });

        let build_land = || {
            let coast_ids = assemble_coastlines(raw.coastlines.clone());
            let chains: Vec<Vec<Point>> = coast_ids.iter().map(|c| resolve(&raw, c)).collect();
            let (background, rings) = land_polygons(&chains, &bounds);
            let mut c = Chunk::default();
            c.push(origin, Kind::Land, 0, 0, 0.0, &rings);
            (background, c)
        };
        let ((background, land), mut chunks) = rayon::join(build_land, || {
            ways.chain(multipolygons)
                .chain(boundaries)
                .chain(points)
                .collect::<Vec<Chunk>>()
        });
        chunks.push(land);

        // Concatenate chunks, rebasing their ring and point indices.
        let total_points = chunks.iter().map(|c| c.points.len()).sum();
        let total_rings = chunks.iter().map(|c| c.ring_starts.len()).sum::<usize>() + 1;
        let mut map_points = Vec::with_capacity(total_points);
        let mut ring_starts = Vec::with_capacity(total_rings);
        let mut features = Vec::new();
        for c in chunks {
            let (p0, r0) = (map_points.len() as u32, ring_starts.len() as u32);
            map_points.extend_from_slice(&c.points);
            ring_starts.extend(c.ring_starts.iter().map(|s| s + p0));
            features.extend(c.features.into_iter().map(|(mut f, area)| {
                f.ring_start += r0;
                (f, area)
            }));
        }
        ring_starts.push(map_points.len() as u32);

        // Draw order: by group; transport by layer; areas largest first so
        // that small parcels sit on top of the large ones containing them.
        features.par_sort_by_key(|(f, area)| {
            let g = f.group();
            let layer = match g {
                g if g.is_transport() => f.layer,
                _ => 0,
            };
            let rank = if g == Group::Areas {
                u32::MAX - area.to_bits()
            } else {
                f.kind as u32
            };
            (g, layer, rank, f.kind, f.flags)
        });
        let features: Vec<Feature> = features.into_iter().map(|(f, _)| f).collect();

        let mut buckets: Vec<Vec<Entry>> = (0..=MAX_BUCKET).map(|_| Vec::new()).collect();
        for (i, f) in features.iter().enumerate() {
            let b = (f.vis_zoom.max(0.0) as usize).min(MAX_BUCKET);
            let r = Rectangle::from_corners([f.bbox[0], f.bbox[1]], [f.bbox[2], f.bbox[3]]);
            buckets[b].push(GeomWithData::new(r, i as u32));
        }
        let index = buckets.into_par_iter().map(RTree::bulk_load).collect();

        let map = Map {
            bounds,
            origin,
            background,
            points: map_points,
            ring_starts,
            features,
            index,
        };
        eprintln!(
            "  built {} features, {} vertices ({:.2?})",
            map.features.len(),
            map.points.len(),
            t.elapsed()
        );
        map
    }

    pub fn ring(&self, r: u32) -> &[[f32; 2]] {
        let (a, b) = (
            self.ring_starts[r as usize],
            self.ring_starts[r as usize + 1],
        );
        &self.points[a as usize..b as usize]
    }

    pub fn rings(&self, f: &Feature) -> impl Iterator<Item = &[[f32; 2]]> {
        (f.ring_start..f.ring_start + f.ring_count).map(|r| self.ring(r))
    }

    /// Indices of features visible at `zoom` whose bounds intersect `area`
    /// (local coordinates), in draw order.
    pub fn query(&self, area: [f32; 4], zoom: f32, out: &mut Vec<u32>) {
        out.clear();
        let env = AABB::from_corners([area[0], area[1]], [area[2], area[3]]);
        let last = (zoom.max(0.0) as usize).min(MAX_BUCKET);
        for tree in &self.index[..=last] {
            out.extend(
                tree.locate_in_envelope_intersecting(env)
                    .map(|e| e.data)
                    .filter(|&i| self.features[i as usize].vis_zoom <= zoom),
            );
        }
        out.sort_unstable();
    }

    /// Per-kind feature counts, most common first.
    pub fn stats(&self) -> Vec<(Kind, usize)> {
        let mut counts = std::collections::BTreeMap::new();
        for f in &self.features {
            *counts.entry(f.kind).or_insert(0usize) += 1;
        }
        let mut v: Vec<_> = counts.into_iter().collect();
        v.sort_by_key(|a| std::cmp::Reverse(a.1));
        v
    }
}
