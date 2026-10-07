//! The render-ready map: flat geometry arenas, features pre-sorted into draw
//! order, and a spatial index bucketed by the zoom at which features appear.

use crate::assemble::{
    assemble_coastlines, assemble_rings, land_polygons, orient_rings, Background,
};
use crate::classify::{flags, Detail, Group, Kind};
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
    /// Top above the ground in meters (admin level for boundaries).
    pub height: f32,
    /// Bottom above the ground in meters: `min_height`, or the roof an
    /// object stands on.
    pub base: f32,
    /// Index into [`Map::details`] for solids with roof/facade detail.
    pub detail: u32,
    /// Model variant of a point object (e.g. a tower's type).
    pub variant: u8,
    /// Compass heading (radians) a point object's front faces; `NaN` when
    /// it has no particular orientation.
    pub heading: f32,
    /// For roads and rails that leave the ground: offset into
    /// [`Map::elevations`] of their per-vertex elevations; `u32::MAX` if they
    /// stay on the ground.
    pub elev: u32,
    /// For a `building:part`: index into [`Map::groups`], the footprint of
    /// the building it belongs to; `u32::MAX` otherwise.
    pub group: u32,
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
    /// Top of the tallest structure, in meters.
    pub max_height: f32,
    /// Roof and facade detail of solids, indexed by [`Feature::detail`].
    pub details: Vec<Detail>,
    /// Per-vertex elevations (meters) of raised roads and rails, smoothed
    /// into ramps; see [`crate::elevation`].
    pub elevations: Vec<f32>,
    /// Bounding boxes of buildings drawn through their parts, indexed by
    /// [`Feature::group`]: parts of one building sort together.
    pub groups: Vec<[f32; 4]>,
    index: Vec<RTree<Entry>>,
}

/// Per-feature attributes passed through map building.
#[derive(Clone, Copy)]
struct Attrs {
    kind: Kind,
    flags: u8,
    layer: i8,
    height: f32,
    base: f32,
    detail: Option<Detail>,
    variant: u8,
    heading: f32,
}

impl Attrs {
    fn new(kind: Kind, flags: u8, layer: i8, height: f32, base: f32) -> Self {
        Attrs {
            kind,
            flags,
            layer,
            height,
            base,
            detail: None,
            variant: 0,
            heading: f32::NAN,
        }
    }

    fn with_detail(mut self, detail: Option<Detail>) -> Self {
        self.detail = detail;
        self
    }
}

/// Accumulates features for one chunk of input; chunks build in parallel and
/// are concatenated afterwards.
#[derive(Default)]
struct Chunk {
    points: Vec<[f32; 2]>,
    ring_starts: Vec<u32>,
    features: Vec<(Feature, f32)>,
    details: Vec<Detail>,
    elevations: Vec<f32>,
}

impl Chunk {
    fn push(&mut self, origin: Point, a: Attrs, rings: &[Vec<Point>]) {
        let Attrs {
            kind,
            flags,
            layer,
            height,
            base,
            detail,
            variant,
            heading,
        } = a;
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
        // Tall, thin things (spires, towers) are visible by their height.
        let tall = if kind.is_extrusion() {
            height as f64 / crate::geo::meters_per_unit(bbox.center()[1])
        } else {
            0.0
        };
        let extent = bbox.width().max(bbox.height()).max(tall * 0.5);
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
        if kind.is_extrusion() {
            // Tall structures stay visible further out, as the skyline.
            vis_zoom = vis_zoom.min(match height {
                h if h >= 80.0 => 9.0,
                h if h >= 40.0 => 10.5,
                h if h >= 20.0 => 12.0,
                _ => f32::INFINITY,
            });
        }
        if !vis_zoom.is_finite() {
            self.ring_starts.truncate(ring_start as usize);
            return;
        }
        let local = |v: f64, o: f64| (v - o) as f32;
        let detail = match detail {
            Some(d) => {
                self.details.push(d);
                self.details.len() as u32 - 1
            }
            None => u32::MAX,
        };
        self.features.push((
            Feature {
                kind,
                flags,
                layer,
                ring_start,
                ring_count,
                height,
                base,
                detail,
                variant,
                heading,
                elev: u32::MAX,
                group: u32::MAX,
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
/// Like [`resolve`], keeping each kept node's elevation alongside.
fn resolve_elevated(raw: &RawData, refs: &[i64], elev: &[f32]) -> (Vec<Point>, Vec<f32>) {
    let mut pts: Vec<Point> = Vec::with_capacity(refs.len());
    let mut out = Vec::with_capacity(refs.len());
    for (&id, &e) in refs.iter().zip(elev) {
        if let Some(p) = raw.node(id) {
            if pts.last() != Some(&p) {
                pts.push(p);
                out.push(e);
            }
        }
    }
    (pts, out)
}

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

/// Even-odd containment of `p` in a feature's rings.
fn contains(rings: &[&[[f32; 2]]], p: [f32; 2]) -> bool {
    let mut inside = false;
    for ring in rings {
        let n = ring.len();
        let mut j = n.wrapping_sub(1);
        for i in 0..n {
            let ([xi, yi], [xj, yj]) = (ring[i], ring[j]);
            if (yi > p[1]) != (yj > p[1]) && p[0] < (xj - xi) * (p[1] - yi) / (yj - yi) + xi {
                inside = !inside;
            }
            j = i;
        }
    }
    inside
}

/// Whether a point object found inside a building footprint is on its roof.
/// Bell towers, minarets and the like are part of their building and rise
/// from the ground; antennas, masts, flagpoles and tanks stand on top.
fn stands_on_roofs(f: &Feature) -> bool {
    use crate::classify::TowerType;
    match f.kind {
        Kind::Tower => matches!(
            TowerType::from_u8(f.variant),
            TowerType::Generic | TowerType::Monopole | TowerType::Lattice | TowerType::Guyed
        ),
        Kind::Mast | Kind::Flagpole | Kind::WaterTower | Kind::Chimney | Kind::Windsock => true,
        k => k.is_point() && !matches!(k, Kind::Tree | Kind::Shrub),
    }
}

/// Places structures relative to each other, as mapped:
///
/// - A building split into `building:part`s is drawn through its parts; its
///   outline (which would swallow them) is dropped.
/// - Objects tagged `location=roof`, and point objects mapped inside a
///   building's footprint (antennas, flagpoles, water tanks rarely carry the
///   tag), stand on the highest flat roof beneath them; pitched roofs and
///   spires are not something to stand on.
fn place_structures(
    features: &mut Vec<(Feature, f32)>,
    points: &[[f32; 2]],
    starts: &[u32],
    details: &[Detail],
) -> Vec<[f32; 4]> {
    let rings_of = |f: &Feature| -> Vec<&[[f32; 2]]> {
        (f.ring_start..f.ring_start + f.ring_count)
            .map(|r| &points[starts[r as usize] as usize..starts[r as usize + 1] as usize])
            .collect()
    };
    // A point standing for the feature: its first ring's vertex average.
    let anchor = |f: &Feature| -> [f32; 2] {
        let ring = &points
            [starts[f.ring_start as usize] as usize..starts[f.ring_start as usize + 1] as usize];
        let n = ring.len() as f32;
        let (sx, sy) = ring
            .iter()
            .fold((0.0, 0.0), |(x, y), p| (x + p[0], y + p[1]));
        [sx / n, sy / n]
    };
    let buildings: Vec<GeomWithData<Rectangle<[f32; 2]>, usize>> = features
        .iter()
        .enumerate()
        .filter(|(_, (f, _))| f.kind == Kind::Building)
        .map(|(i, (f, _))| {
            let r = Rectangle::from_corners([f.bbox[0], f.bbox[1]], [f.bbox[2], f.bbox[3]]);
            GeomWithData::new(r, i)
        })
        .collect();
    if buildings.is_empty() {
        return Vec::new();
    }
    let index = RTree::bulk_load(buildings);

    // (part, outline) pairs.
    let membership: Vec<(usize, usize)> = features
        .par_iter()
        .enumerate()
        .filter(|(_, (f, _))| f.flags & flags::PART != 0)
        .filter_map(|(i, (part, _))| {
            let p = anchor(part);
            index
                .locate_all_at_point(p)
                .find(|e| {
                    let outline = &features[e.data].0;
                    outline.flags & flags::PART == 0 && contains(&rings_of(outline), p)
                })
                .map(|e| (i, e.data))
        })
        .collect();
    let mut is_hidden = vec![false; features.len()];
    let mut groups: Vec<[f32; 4]> = Vec::new();
    let mut group_of: rustc_hash::FxHashMap<usize, u32> = Default::default();
    for &(part, outline) in &membership {
        is_hidden[outline] = true;
        let g = *group_of.entry(outline).or_insert_with(|| {
            groups.push(features[outline].0.bbox);
            groups.len() as u32 - 1
        });
        features[part].0.group = g;
    }

    let lifts: Vec<(usize, f32)> = features
        .par_iter()
        .enumerate()
        .filter(|(_, (f, _))| f.flags & flags::ON_ROOF != 0 || stands_on_roofs(f))
        .filter_map(|(i, (f, _))| {
            let p = anchor(f);
            let (mut flat, mut any) = (0.0_f32, 0.0_f32);
            for e in index.locate_all_at_point(p) {
                if e.data == i || is_hidden[e.data] {
                    continue;
                }
                let b = &features[e.data].0;
                if !contains(&rings_of(b), p) {
                    continue;
                }
                any = any.max(b.height);
                let pitched = details
                    .get(b.detail as usize)
                    .is_some_and(|d| d.roof != crate::classify::RoofShape::Flat);
                if !pitched {
                    flat = flat.max(b.height);
                }
            }
            let roof = if flat > 0.0 { flat } else { any };
            (roof > 0.0).then_some((i, roof))
        })
        .collect();
    for (i, roof) in lifts {
        let f = &mut features[i].0;
        f.base += roof;
        f.height += roof;
    }

    let mut i = 0;
    features.retain(|_| {
        i += 1;
        !is_hidden[i - 1]
    });
    groups
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

        // Roads and rails climb smoothly to their bridges.
        let profiles = crate::elevation::profiles(&raw);
        let ways = raw.ways.par_chunks(4096).enumerate().map(|(n, chunk)| {
            let mut c = Chunk::default();
            for (k, w) in chunk.iter().enumerate() {
                let attrs =
                    Attrs::new(w.kind, w.flags, w.layer, w.height, w.base).with_detail(w.detail);
                if let Some(elev) = profiles.get(&(n * 4096 + k)) {
                    let (pts, elev) = resolve_elevated(&raw, &w.refs, elev);
                    if pts.len() >= 2 {
                        let before = c.features.len();
                        c.push(origin, attrs, &[pts]);
                        if c.features.len() > before {
                            c.features[before].0.elev = c.elevations.len() as u32;
                            c.elevations.extend(elev);
                        }
                    }
                    continue;
                }
                let pts = resolve(&raw, &w.refs);
                if w.kind.is_area() {
                    let mut rings = [open_ring(pts)];
                    if rings[0].len() < 3 {
                        continue;
                    }
                    orient_rings(&mut rings);
                    c.push(origin, attrs, &rings);
                } else if pts.len() >= 2 {
                    c.push(origin, attrs, &[pts]);
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
                let attrs = Attrs::new(mp.kind, mp.flags, mp.layer, mp.height, mp.base)
                    .with_detail(mp.detail);
                c.push(origin, attrs, &rings);
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
                            let attrs = Attrs::new(Kind::Boundary, 0, 0, level as f32, 0.0);
                            c.push(origin, attrs, &[pts]);
                        }
                    }
                    c
                });

        // Street furniture goes where it stands, turned the way it faces.
        let placed = crate::place::place(&raw);
        let points = placed.par_chunks(65536).map(|chunk| {
            let mut c = Chunk::default();
            for placed in chunk {
                let p = &placed.point;
                let mut attrs = Attrs::new(p.kind, p.flags, 0, p.height, 0.0);
                attrs.variant = p.variant;
                attrs.heading = placed.heading;
                c.push(origin, attrs, &[vec![p.at]]);
            }
            c
        });

        let build_land = || {
            let coast_ids = assemble_coastlines(raw.coastlines.clone());
            let chains: Vec<Vec<Point>> = coast_ids.iter().map(|c| resolve(&raw, c)).collect();
            let (background, rings) = land_polygons(&chains, &bounds);
            let mut c = Chunk::default();
            c.push(origin, Attrs::new(Kind::Land, 0, 0, 0.0, 0.0), &rings);
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
        let mut details = Vec::new();
        let mut elevations = Vec::new();
        for c in chunks {
            let (p0, r0) = (map_points.len() as u32, ring_starts.len() as u32);
            let (d0, e0) = (details.len() as u32, elevations.len() as u32);
            map_points.extend_from_slice(&c.points);
            ring_starts.extend(c.ring_starts.iter().map(|s| s + p0));
            details.extend_from_slice(&c.details);
            elevations.extend_from_slice(&c.elevations);
            features.extend(c.features.into_iter().map(|(mut f, area)| {
                f.ring_start += r0;
                if f.detail != u32::MAX {
                    f.detail += d0;
                }
                if f.elev != u32::MAX {
                    f.elev += e0;
                }
                (f, area)
            }));
        }
        ring_starts.push(map_points.len() as u32);
        let groups = place_structures(&mut features, &map_points, &ring_starts, &details);
        let max_height = features
            .iter()
            .filter(|(f, _)| f.kind.is_extrusion() || f.kind.is_point())
            .map(|(f, _)| f.height)
            .fold(0.0_f32, f32::max);

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

        let index = build_index(&features);

        let map = Map {
            bounds,
            origin,
            background,
            points: map_points,
            ring_starts,
            features,
            max_height,
            details,
            elevations,
            groups,
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

    /// A map from its parts, as read back from a snapshot.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        bounds: Rect,
        origin: Point,
        background: Background,
        points: Vec<[f32; 2]>,
        ring_starts: Vec<u32>,
        features: Vec<Feature>,
        max_height: f32,
        details: Vec<Detail>,
        elevations: Vec<f32>,
        groups: Vec<[f32; 4]>,
    ) -> Map {
        let index = build_index(&features);
        Map {
            bounds,
            origin,
            background,
            points,
            ring_starts,
            features,
            max_height,
            details,
            elevations,
            groups,
            index,
        }
    }

    /// Per-vertex elevations (meters) of a raised road or rail, if any.
    pub fn elevations(&self, f: &Feature) -> Option<&[f32]> {
        if f.elev == u32::MAX {
            return None;
        }
        let start = f.elev as usize;
        let n = self.rings(f).map(<[_]>::len).sum::<usize>();
        self.elevations.get(start..start + n)
    }

    /// Roof and facade detail of a solid, if mapped.
    pub fn detail(&self, f: &Feature) -> Detail {
        self.details
            .get(f.detail as usize)
            .copied()
            .unwrap_or_default()
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

/// One R-tree per zoom bucket: a feature lives in the bucket of the zoom it
/// appears at, so a query walks only the buckets up to its zoom.
fn build_index(features: &[Feature]) -> Vec<RTree<Entry>> {
    let mut buckets: Vec<Vec<Entry>> = (0..=MAX_BUCKET).map(|_| Vec::new()).collect();
    for (i, f) in features.iter().enumerate() {
        let b = (f.vis_zoom.max(0.0) as usize).min(MAX_BUCKET);
        let r = Rectangle::from_corners([f.bbox[0], f.bbox[1]], [f.bbox[2], f.bbox[3]]);
        buckets[b].push(GeomWithData::new(r, i as u32));
    }
    buckets.into_par_iter().map(RTree::bulk_load).collect()
}
