//! Parallel `.osm.pbf` reader.
//!
//! PBF files store nodes, then ways, then relations, but multipolygon member
//! ways are often untagged and only identifiable through their relation, and
//! ways reference nodes that came earlier. Rather than holding every node of
//! the file in memory, ingest runs three passes over the compressed blobs:
//!
//! 1. **Relations** — find multipolygons and boundaries and the ways they use.
//!    This pass also records which blobs hold ways and which hold nodes.
//! 2. **Ways** — keep renderable ways, coastlines and relation members, as node
//!    ID lists. Node blobs are skipped without being decompressed.
//! 3. **Nodes** — look up coordinates only for the node IDs collected in pass
//!    2, plus tagged point features. Way blobs are skipped.
//!
//! Each pass decodes blobs on all cores.

use crate::classify::{boundary_level, node_kind, way_kind, Kind, Tags};
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
    pub height: f32,
    pub refs: Vec<i64>,
}

/// A multipolygon relation: an area made of several member ways.
pub struct RawMultipolygon {
    pub id: i64,
    pub kind: Kind,
    pub flags: u8,
    pub layer: i8,
    pub height: f32,
    pub members: Vec<i64>,
}

/// Everything the map builder needs, still keyed by OSM IDs.
pub struct RawData {
    /// Bounding box from the file header, if present.
    pub header_bbox: Option<Rect>,
    /// Bounding box of every node in the file.
    pub node_bbox: Rect,
    /// Sorted, unique IDs of every node referenced by a kept way.
    pub node_ids: Vec<i64>,
    /// Projected coordinate of `node_ids[i]` (NaN when missing from the file).
    pub node_xy: Vec<Point>,
    pub ways: Vec<RawWay>,
    pub multipolygons: Vec<RawMultipolygon>,
    /// Node refs of ways used by relations, by way ID.
    pub member_ways: FxHashMap<i64, Vec<i64>>,
    /// Ways of administrative boundaries, with the lowest admin level using them.
    pub boundary_ways: FxHashMap<i64, u8>,
    pub coastlines: Vec<Vec<i64>>,
    pub points: Vec<(Kind, Point)>,
}

impl RawData {
    /// Coordinates of a node, or `None` if it is not in the file.
    pub fn node(&self, id: i64) -> Option<Point> {
        let i = self.node_ids.binary_search(&id).ok()?;
        let p = self.node_xy[i];
        (!p[0].is_nan()).then_some(p)
    }
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
    ways: Vec<RawWay>,
    members: FxHashMap<i64, Vec<i64>>,
    coastlines: Vec<Vec<i64>>,
}

#[derive(Default)]
struct NodePass {
    hits: Vec<(u32, Point)>,
    points: Vec<(Kind, Point)>,
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
                                acc.multipolygons.push(RawMultipolygon {
                                    id: r.id(),
                                    kind,
                                    flags: tags.flags(),
                                    layer: tags.layer(),
                                    height: tags.height().unwrap_or(0.0),
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
                    let refs: Vec<i64> = w.refs().collect();
                    if coast {
                        acc.coastlines.push(refs.clone());
                    }
                    if is_member {
                        acc.members.insert(w.id(), refs.clone());
                    }
                    if let Some(kind) = kind {
                        acc.ways.push(RawWay {
                            id: w.id(),
                            kind,
                            flags: tags.flags(),
                            layer: tags.layer(),
                            height: tags.height().unwrap_or(0.0),
                            refs,
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
        .flat_map_iter(|w| w.refs.iter().copied())
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
    let ids = &node_ids;

    let nodes = par_blocks(
        path,
        |i| blob_info.get(i).is_some_and(|b| b.nodes),
        |acc: &mut NodePass, _, block| {
            // Node IDs ascend within a block, so walk a cursor through the
            // sorted wanted list instead of binary-searching every node.
            let mut cursor: Option<usize> = None;
            let mut last_id = i64::MIN;
            let mut bbox = acc.bbox.unwrap_or(Rect::EMPTY);
            let mut visit = |id: i64, lon: f64, lat: f64, tagged: Option<Kind>| {
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
                    acc.hits.push((c as u32, p));
                }
                if let Some(kind) = tagged {
                    acc.points.push((kind, p));
                }
            };
            for group in block.groups() {
                for n in group.dense_nodes() {
                    let tagged = if n.raw_tags().len() > 0 {
                        node_kind(&Tags::parse(n.tags()))
                    } else {
                        None
                    };
                    visit(n.id(), n.lon(), n.lat(), tagged);
                }
                for n in group.nodes() {
                    visit(n.id(), n.lon(), n.lat(), node_kind(&Tags::parse(n.tags())));
                }
            }
            acc.bbox = Some(bbox);
        },
        |mut a, b| {
            a.hits.extend(b.hits);
            a.points.extend(b.points);
            a.bbox = match (a.bbox, b.bbox) {
                (Some(x), Some(y)) => Some(x.union(&y)),
                (x, y) => x.or(y),
            };
            a
        },
    )?;
    let mut node_xy = vec![[f64::NAN; 2]; node_ids.len()];
    for (i, p) in nodes.hits {
        node_xy[i as usize] = p;
    }
    eprintln!(
        "  nodes: {} referenced, {} point features ({:.2?})",
        node_ids.len(),
        nodes.points.len(),
        t.elapsed()
    );

    // Blobs finish in arbitrary order; sort so output is reproducible.
    let (mut ways_out, mut multipolygons, mut coastlines, mut points) =
        (ways.ways, rel.multipolygons, ways.coastlines, nodes.points);
    ways_out.par_sort_unstable_by_key(|w| w.id);
    multipolygons.par_sort_unstable_by_key(|m| m.id);
    coastlines.sort_unstable_by_key(|c| c[0]);
    points.par_sort_unstable_by(|a, b| a.1[0].total_cmp(&b.1[0]).then(a.1[1].total_cmp(&b.1[1])));

    Ok(RawData {
        header_bbox,
        node_bbox: nodes.bbox.unwrap_or(Rect::EMPTY),
        node_ids,
        node_xy,
        ways: ways_out,
        multipolygons,
        member_ways: ways.members,
        boundary_ways: rel.boundary_ways,
        coastlines,
        points,
    })
}
