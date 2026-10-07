//! Parallel `.osm.pbf` reader, in three passes over the blobs:
//!
//! 1. Relations: multipolygons, boundaries and their member ways.
//! 2. Ways: renderable ways as node ID lists (node blobs skipped).
//! 3. Nodes: coordinates for the collected IDs plus tagged points.

use crate::classify::{boundary_level, node_kind, way_kind, Detail, Facing, Kind, Tags};
use crate::geo::{project, Point, Rect};
use anyhow::{Context, Result};
use osmpbf::{BlobDecode, BlobReader, BlobType, PrimitiveBlock, RelMemberType};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

/// A way that renders as a feature, with its node references.
pub struct RawWay {
    pub id: i64,
    pub kind: Kind,
    pub flags: u8,
    pub layer: i8,
    /// Top above the ground in meters (tagged, or typical for the kind).
    pub height: f32,
    /// Bottom above the ground in meters (`min_height`).
    pub base: f32,
    /// Roof and facade detail, for extruded solids (boxed: most ways have
    /// none, and there are tens of millions of ways).
    pub detail: Option<Box<Detail>>,
    /// Indices into [`RawData::node_xy`].
    pub refs: Box<[u32]>,
}

/// A way as read, before node IDs become indices.
struct PendingWay {
    way: RawWay,
    ids: Box<[i64]>,
}

/// A physical object mapped as a single node.
#[derive(Clone, Copy)]
pub struct RawPoint {
    /// OSM node ID.
    pub id: i64,
    /// Index into [`RawData::node_xy`] if a kept way uses this node, to find
    /// the ways the object stands on.
    pub node: Option<u32>,
    pub kind: Kind,
    pub flags: u8,
    /// Model variant (e.g. a tower's type).
    pub variant: u8,
    pub height: f32,
    /// Mapped `direction`.
    pub facing: Facing,
    pub at: Point,
}

/// Height and base of a feature from its tags, falling back to typical
/// dimensions for its kind.
fn dimensions(kind: Kind, tags: &Tags) -> (f32, f32) {
    let height = tags.height().unwrap_or_else(|| {
        let typical = kind.default_height(tags);
        // Rooftop tanks and towers are much smaller than freestanding ones.
        if tags.flags() & crate::classify::flags::ON_ROOF != 0 {
            typical.min(6.0)
        } else {
            typical
        }
    });
    (height, tags.min_height().min(height))
}

/// A multipolygon relation: an area made of several member ways.
pub struct RawMultipolygon {
    pub id: i64,
    pub kind: Kind,
    pub flags: u8,
    pub layer: i8,
    pub height: f32,
    pub base: f32,
    pub detail: Option<Detail>,
    pub members: Vec<i64>,
}

/// Everything the map builder needs, still keyed by OSM IDs.
pub struct RawData {
    /// Bounding box from the file header, if present.
    pub header_bbox: Option<Rect>,
    /// Bounding box of every node in the file.
    pub node_bbox: Rect,
    /// Coordinate of every node a kept way uses, packed by [`pack`], indexed
    /// by the node references below; [`MISSING`] when not in the file.
    pub node_xy: Vec<u64>,
    pub ways: Vec<RawWay>,
    pub multipolygons: Vec<RawMultipolygon>,
    /// Node references of ways used by relations, by way ID.
    pub member_ways: FxHashMap<i64, Box<[u32]>>,
    /// Ways of administrative boundaries, with the lowest admin level using them.
    pub boundary_ways: FxHashMap<i64, u8>,
    pub coastlines: Vec<Vec<u32>>,
    pub points: Vec<RawPoint>,
}

impl RawData {
    /// Coordinates of a node, or `None` if it is not in the file.
    pub fn node(&self, i: u32) -> Option<Point> {
        unpack(self.node_xy[i as usize])
    }
}

/// Marks a referenced node that the file does not contain.
pub const MISSING: u64 = u64::MAX;
const FIXED: f64 = 4294967296.0;

/// A normalized coordinate as two 32-bit fixed-point halves: about 1 cm,
/// half the memory of two f64s.
pub fn pack([x, y]: Point) -> u64 {
    let q = |v: f64| (v * FIXED).clamp(0.0, FIXED - 2.0) as u64;
    q(x) << 32 | q(y)
}

pub fn unpack(v: u64) -> Option<Point> {
    (v != MISSING).then(|| [(v >> 32) as f64 / FIXED, (v & 0xffff_ffff) as f64 / FIXED])
}

#[derive(Clone, Copy, Default)]
struct BlobInfo {
    nodes: bool,
    ways: bool,
}

/// Decodes the blobs selected by `want` in parallel, folding each primitive
/// block into a per-thread accumulator and merging them at the end.
fn par_blocks<T, W, F, M>(path: &Path, want: W, fold: F, merge: M) -> Result<T>
where
    T: Default + Send,
    W: Fn(usize) -> bool + Sync,
    F: Fn(&mut T, usize, &PrimitiveBlock) + Sync,
    M: Fn(T, T) -> T + Sync,
{
    let reader =
        BlobReader::from_path(path).with_context(|| format!("opening {}", path.display()))?;
    let first_error = Mutex::new(None);
    let result = reader
        .enumerate()
        .filter(|(i, _)| want(*i))
        .par_bridge()
        .fold(T::default, |mut acc, (i, blob)| {
            let block = match blob {
                Ok(b) if b.get_type() == BlobType::OsmData => b.to_primitiveblock(),
                Ok(_) => return acc,
                Err(e) => Err(e),
            };
            match block {
                Ok(block) => fold(&mut acc, i, &block),
                Err(e) => {
                    first_error.lock().unwrap().get_or_insert(e);
                }
            }
            acc
        })
        .reduce(T::default, &merge);
    match first_error.into_inner().unwrap() {
        Some(e) => Err(e).context("decoding PBF blob"),
        None => Ok(result),
    }
}

fn read_header_bbox(path: &Path) -> Result<Option<Rect>> {
    let mut reader = BlobReader::from_path(path)?;
    if let Some(blob) = reader.next() {
        if let BlobDecode::OsmHeader(header) = blob?.decode()? {
            return Ok(header.bbox().map(|b| {
                Rect::from_lon_lat(
                    b.left.min(b.right),
                    b.top.min(b.bottom),
                    b.left.max(b.right),
                    b.top.max(b.bottom),
                )
            }));
        }
    }
    Ok(None)
}

#[derive(Default)]
struct RelationPass {
    blobs: Vec<(usize, BlobInfo)>,
    multipolygons: Vec<RawMultipolygon>,
    boundary_ways: FxHashMap<i64, u8>,
}

#[derive(Default)]
struct WayPass {
    ways: Vec<PendingWay>,
    members: FxHashMap<i64, Box<[i64]>>,
    coastlines: Vec<Box<[i64]>>,
}

#[derive(Default)]
struct NodePass {
    points: Vec<RawPoint>,
    bbox: Option<Rect>,
}

pub fn read(path: &Path) -> Result<RawData> {
    let header_bbox = read_header_bbox(path)?;

    // Pass 1: relations, plus a map of which blobs hold what.
    let t = Instant::now();
    let rel = par_blocks(
        path,
        |_| true,
        |acc: &mut RelationPass, i, block| {
            let mut info = BlobInfo::default();
            for group in block.groups() {
                info.nodes |= group.dense_nodes().len() > 0 || group.nodes().len() > 0;
                info.ways |= group.ways().len() > 0;
                for r in group.relations() {
                    let tags = Tags::parse(r.tags());
                    let ways = || {
                        r.members()
                            .filter(|m| m.member_type == RelMemberType::Way)
                            .map(|m| m.member_id)
                    };
                    match tags.kind {
                        Some("multipolygon") => {
                            if let Some(kind) = crate::classify::area_kind(&tags) {
                                let (height, base) = dimensions(kind, &tags);
                                acc.multipolygons.push(RawMultipolygon {
                                    id: r.id(),
                                    kind,
                                    flags: tags.flags(),
                                    layer: tags.layer(),
                                    height,
                                    base,
                                    detail: kind.is_extrusion().then(|| tags.detail()),
                                    members: ways().collect(),
                                });
                            }
                        }
                        Some("boundary") => {
                            if let Some(level) = boundary_level(&tags) {
                                for id in ways() {
                                    let e = acc.boundary_ways.entry(id).or_insert(level);
                                    *e = (*e).min(level);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            acc.blobs.push((i, info));
        },
        |mut a, b| {
            a.blobs.extend(b.blobs);
            a.multipolygons.extend(b.multipolygons);
            for (id, level) in b.boundary_ways {
                let e = a.boundary_ways.entry(id).or_insert(level);
                *e = (*e).min(level);
            }
            a
        },
    )?;
    let mut blob_info =
        vec![BlobInfo::default(); rel.blobs.iter().map(|b| b.0 + 1).max().unwrap_or(0)];
    for (i, info) in &rel.blobs {
        blob_info[*i] = *info;
    }
    let member_ids: FxHashSet<i64> = rel
        .multipolygons
        .iter()
        .flat_map(|m| m.members.iter().copied())
        .chain(rel.boundary_ways.keys().copied())
        .collect();
    eprintln!(
        "  relations: {} multipolygons, {} boundary ways ({:.2?})",
        rel.multipolygons.len(),
        rel.boundary_ways.len(),
        t.elapsed()
    );

    // Pass 2: ways.
    let t = Instant::now();
    let ways = par_blocks(
        path,
        |i| blob_info.get(i).is_some_and(|b| b.ways),
        |acc: &mut WayPass, _, block| {
            for group in block.groups() {
                for w in group.ways() {
                    // Refs are delta-coded: the way is closed when the deltas
                    // after the first sum to zero.
                    let deltas = w.raw_refs();
                    if deltas.len() < 2 {
                        continue;
                    }
                    let tags = Tags::parse(w.tags());
                    let is_member = member_ids.contains(&w.id());
                    let closed = deltas.len() >= 4 && deltas[1..].iter().sum::<i64>() == 0;
                    let kind = way_kind(&tags, closed);
                    let coast = tags.natural == Some("coastline");
                    if kind.is_none() && !coast && !is_member {
                        continue;
                    }
                    let refs: Box<[i64]> = w.refs().collect();
                    if coast {
                        acc.coastlines.push(refs.clone());
                    }
                    if is_member {
                        acc.members.insert(w.id(), refs.clone());
                    }
                    if let Some(kind) = kind {
                        let (height, base) = dimensions(kind, &tags);
                        acc.ways.push(PendingWay {
                            way: RawWay {
                                id: w.id(),
                                kind,
                                flags: tags.flags(),
                                layer: tags.layer(),
                                height,
                                base,
                                detail: kind.is_extrusion().then(|| Box::new(tags.detail())),
                                refs: Box::default(),
                            },
                            ids: refs,
                        });
                    }
                }
            }
        },
        |mut a, b| {
            a.ways.extend(b.ways);
            a.members.extend(b.members);
            a.coastlines.extend(b.coastlines);
            a
        },
    )?;
    eprintln!(
        "  ways: {} features, {} relation members, {} coastline ways ({:.2?})",
        ways.ways.len(),
        ways.members.len(),
        ways.coastlines.len(),
        t.elapsed()
    );

    // Pass 3: node coordinates for everything referenced above.
    let t = Instant::now();
    let mut node_ids: Vec<i64> = ways
        .ways
        .par_iter()
        .flat_map_iter(|w| w.ids.iter().copied())
        .chain(
            ways.members
                .par_iter()
                .flat_map_iter(|(_, r)| r.iter().copied()),
        )
        .chain(
            ways.coastlines
                .par_iter()
                .flat_map_iter(|r| r.iter().copied()),
        )
        .collect();
    node_ids.par_sort_unstable();
    node_ids.dedup();
    node_ids.shrink_to_fit();
    let ids = &node_ids;
    // Each referenced node's slot is written once, from whichever thread
    // decodes it, so no list of hits is ever held.
    let xy: Vec<std::sync::atomic::AtomicU64> = (0..node_ids.len())
        .map(|_| std::sync::atomic::AtomicU64::new(MISSING))
        .collect();
    let slots = &xy;

    let nodes = par_blocks(
        path,
        |i| blob_info.get(i).is_some_and(|b| b.nodes),
        |acc: &mut NodePass, _, block| {
            // Node IDs ascend within a block, so walk a cursor through the
            // sorted wanted list instead of binary-searching every node.
            let mut cursor: Option<usize> = None;
            let mut last_id = i64::MIN;
            let mut bbox = acc.bbox.unwrap_or(Rect::EMPTY);
            let mut visit =
                |id: i64, lon: f64, lat: f64, tagged: Option<(Kind, u8, u8, f32, Facing)>| {
                    let c = match cursor {
                        Some(mut c) if id >= last_id => {
                            while c < ids.len() && ids[c] < id {
                                c += 1;
                            }
                            c
                        }
                        _ => ids.partition_point(|&x| x < id),
                    };
                    cursor = Some(c);
                    last_id = id;
                    let p = project(lon, lat);
                    bbox.extend(p);
                    if c < ids.len() && ids[c] == id {
                        slots[c].store(pack(p), std::sync::atomic::Ordering::Relaxed);
                    }
                    if let Some((kind, flags, variant, height, facing)) = tagged {
                        acc.points.push(RawPoint {
                            id,
                            node: None,
                            kind,
                            flags,
                            variant,
                            height,
                            facing,
                            at: p,
                        });
                    }
                };
            let object = |tags: Tags| {
                node_kind(&tags).map(|k| {
                    let height = dimensions(k, &tags).0;
                    (k, tags.flags(), tags.variant(), height, tags.facing())
                })
            };
            for group in block.groups() {
                for n in group.dense_nodes() {
                    let tagged = if n.raw_tags().len() > 0 {
                        object(Tags::parse(n.tags()))
                    } else {
                        None
                    };
                    visit(n.id(), n.lon(), n.lat(), tagged);
                }
                for n in group.nodes() {
                    visit(n.id(), n.lon(), n.lat(), object(Tags::parse(n.tags())));
                }
            }
            acc.bbox = Some(bbox);
        },
        |mut a, b| {
            a.points.extend(b.points);
            a.bbox = match (a.bbox, b.bbox) {
                (Some(x), Some(y)) => Some(x.union(&y)),
                (x, y) => x.or(y),
            };
            a
        },
    )?;
    let node_xy: Vec<u64> = xy.into_iter().map(|a| a.into_inner()).collect();
    eprintln!(
        "  nodes: {} referenced, {} point features ({:.2?})",
        node_ids.len(),
        nodes.points.len(),
        t.elapsed()
    );

    // Node IDs become indices into node_xy, and the IDs are dropped.
    let index = |id: i64| {
        node_ids
            .binary_search(&id)
            .expect("every ref was collected") as u32
    };
    let indices = |ids: &[i64]| -> Box<[u32]> { ids.iter().map(|&id| index(id)).collect() };
    let mut ways_out: Vec<RawWay> = ways
        .ways
        .into_par_iter()
        .map(|p| RawWay {
            refs: indices(&p.ids),
            ..p.way
        })
        .collect();
    let member_ways: FxHashMap<i64, Box<[u32]>> = ways
        .members
        .into_par_iter()
        .map(|(id, ids)| (id, indices(&ids)))
        .collect();
    let mut coastlines: Vec<Vec<u32>> = ways
        .coastlines
        .into_par_iter()
        .map(|ids| indices(&ids).into_vec())
        .collect();
    let mut points = nodes.points;
    points.par_iter_mut().for_each(|p| {
        p.node = node_ids.binary_search(&p.id).ok().map(|i| i as u32);
    });
    drop(node_ids);

    // Blobs finish in arbitrary order; sort so output is reproducible.
    let mut multipolygons = rel.multipolygons;
    ways_out.par_sort_unstable_by_key(|w| w.id);
    multipolygons.par_sort_unstable_by_key(|m| m.id);
    coastlines.sort_unstable_by_key(|c| c[0]);
    points.par_sort_unstable_by(|a, b| {
        (a.at[0].total_cmp(&b.at[0]))
            .then(a.at[1].total_cmp(&b.at[1]))
            .then(a.kind.cmp(&b.kind))
    });

    Ok(RawData {
        header_bbox,
        node_bbox: nodes.bbox.unwrap_or(Rect::EMPTY),
        node_xy,
        ways: ways_out,
        multipolygons,
        member_ways,
        boundary_ways: rel.boundary_ways,
        coastlines,
        points,
    })
}
