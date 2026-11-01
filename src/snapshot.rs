//! Map snapshots: a built [`Map`] saved to disk, so a server starts in
//! seconds instead of rebuilding from an `.osm.pbf`.
//!
//! `MAPSNAP2`, the format written, is memory-mapped and used in place: the
//! arrays are borrowed straight from the file, and the kernel pages them in
//! as tiles touch them and drops them under memory pressure, so serving
//! needs little anonymous memory. Layout, little-endian: a fixed [`Header`]
//! (bounds, origin, background, max height, and the offset and length of
//! each section), then the sections, each 64-byte aligned: points, ring
//! starts, features, details, elevations, groups, per-kind feature counts,
//! and the spatial index (one packed Hilbert R-tree per zoom bucket; see
//! [`crate::index`]).
//!
//! `MAPSNAP1`, the earlier format, is still read: copied onto the heap with
//! the index rebuilt, as it was. [`convert`] turns one into the other.
//!
//! A mapped file must never change while a server has it open: replace a
//! snapshot by writing a new file and renaming it over the old one.

use crate::assemble::Background;
use crate::classify::{Kind, RoofShape};
use crate::geo::Rect;
use crate::index::{self, Packed};
use crate::map::{build_index, count_kinds, DetailRec, Feature, Map, MAX_BUCKET};
use crate::store::Store;
use anyhow::{bail, ensure, Context, Result};
use bytemuck::{Pod, Zeroable};
use memmap2::Mmap;
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[cfg(target_endian = "big")]
compile_error!("snapshots are written as little-endian memory");

pub const MAGIC: &[u8; 8] = b"MAPSNAP2";
pub const MAGIC_V1: &[u8; 8] = b"MAPSNAP1";
const ALIGN: usize = 64;
const HEADER_BYTES: usize = 256;

/// Sections in file order.
const POINTS: usize = 0;
const RING_STARTS: usize = 1;
const FEATURES: usize = 2;
const DETAILS: usize = 3;
const ELEVATIONS: usize = 4;
const GROUPS: usize = 5;
/// `u64` per kind discriminant, so `/v1/kinds` needs no scan.
const KIND_COUNTS: usize = 6;
/// `u64` per zoom bucket: the number of features in its tree.
const INDEX_LENS: usize = 7;
/// Every bucket's tree nodes, bucket after bucket.
const INDEX_BOXES: usize = 8;
const INDEX_IDS: usize = 9;
const SECTIONS: usize = 10;
const ELEMENT_BYTES: [usize; SECTIONS] = [8, 4, 56, 24, 4, 16, 8, 8, 16, 4];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Header {
    magic: [u8; 8],
    bounds: [f64; 4],
    origin: [f64; 2],
    max_height: f32,
    /// 1 for water, 0 for land.
    background: u32,
    /// `[byte offset, element count]` of each section.
    sections: [[u64; 2]; SECTIONS],
}

const _: () = assert!(std::mem::size_of::<Header>() <= HEADER_BYTES);

/// Whether `path` holds a snapshot (either format) rather than an `.osm.pbf`.
pub fn is_snapshot(path: &Path) -> bool {
    let mut head = [0u8; 8];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|_| &head == MAGIC || &head == MAGIC_V1)
}

/// Everything a v2 file holds, as bytes.
struct Parts<'a> {
    bounds: Rect,
    origin: [f64; 2],
    background: Background,
    max_height: f32,
    /// Each section's bytes, possibly in several pieces.
    sections: [Vec<&'a [u8]>; SECTIONS],
}

fn index_sections(index: &[Packed]) -> (Vec<u64>, Vec<&[u8]>, Vec<&[u8]>) {
    let lens = index.iter().map(|t| t.len() as u64).collect();
    let boxes = index
        .iter()
        .map(|t| bytemuck::cast_slice(&t.boxes[..]))
        .collect();
    let ids = index
        .iter()
        .map(|t| bytemuck::cast_slice(&t.ids[..]))
        .collect();
    (lens, boxes, ids)
}

/// Writes `parts` to `path` via a `.partial` file renamed into place, so a
/// server mapping the old file never sees it change.
fn write(path: &Path, parts: &Parts) -> Result<()> {
    let mut partial = path.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    let file = File::create(&partial).with_context(|| format!("creating {}", partial.display()))?;
    let mut w = BufWriter::with_capacity(1 << 22, file);
    w.write_all(&[0u8; HEADER_BYTES])?;
    let mut pos = HEADER_BYTES;
    let mut header = Header {
        magic: *MAGIC,
        bounds: [
            parts.bounds.min_x,
            parts.bounds.min_y,
            parts.bounds.max_x,
            parts.bounds.max_y,
        ],
        origin: parts.origin,
        max_height: parts.max_height,
        background: (parts.background == Background::Water) as u32,
        sections: [[0; 2]; SECTIONS],
    };
    for (s, pieces) in parts.sections.iter().enumerate() {
        let pad = pos.next_multiple_of(ALIGN) - pos;
        w.write_all(&[0u8; ALIGN][..pad])?;
        pos += pad;
        let bytes: usize = pieces.iter().map(|p| p.len()).sum();
        ensure!(
            bytes.is_multiple_of(ELEMENT_BYTES[s]),
            "section {s} is not whole elements"
        );
        header.sections[s] = [pos as u64, (bytes / ELEMENT_BYTES[s]) as u64];
        for p in pieces {
            w.write_all(p)?;
        }
        pos += bytes;
    }
    w.seek(SeekFrom::Start(0))?;
    w.write_all(bytemuck::bytes_of(&header))?;
    let file = w.into_inner().map_err(|e| e.into_error())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&partial, path)
        .with_context(|| format!("renaming {} into place", partial.display()))?;
    Ok(())
}

/// Saves `map` as a v2 snapshot.
pub fn save(map: &Map, path: &Path) -> Result<()> {
    let t = Instant::now();
    let (lens, boxes, ids) = index_sections(&map.index);
    let parts = Parts {
        bounds: map.bounds,
        origin: map.origin,
        background: map.background,
        max_height: map.max_height,
        sections: [
            vec![bytemuck::cast_slice(&map.points[..])],
            vec![bytemuck::cast_slice(&map.ring_starts[..])],
            vec![bytemuck::cast_slice(&map.features[..])],
            vec![bytemuck::cast_slice(&map.details[..])],
            vec![bytemuck::cast_slice(&map.elevations[..])],
            vec![bytemuck::cast_slice(&map.groups[..])],
            vec![bytemuck::cast_slice(&map.kind_counts)],
            vec![bytemuck::cast_slice(&lens)],
            boxes,
            ids,
        ],
    };
    write(path, &parts)?;
    eprintln!("saved {} in {:.2?}", path.display(), t.elapsed());
    Ok(())
}

/// Maps `path` read-only for the rest of the process.
fn map_file(path: &Path) -> Result<&'static [u8]> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    // SAFETY: the file is opened read-only and is never modified while
    // mapped: snapshots are replaced by renaming a new file over them, which
    // leaves this one intact (see the module docs).
    let mmap =
        unsafe { Mmap::map(&file) }.with_context(|| format!("mapping {}", path.display()))?;
    Ok(&Box::leak(Box::new(mmap))[..])
}

/// Loads a snapshot of either format.
pub fn load(path: &Path) -> Result<Map> {
    let t = Instant::now();
    let bytes = map_file(path)?;
    let map = match bytes.get(..8) {
        Some(m) if m == MAGIC => open_v2(bytes)?,
        Some(m) if m == MAGIC_V1 => load_v1(bytes)?,
        _ => bail!("{} is not a map snapshot", path.display()),
    };
    let (heap, mapped) = map.storage();
    eprintln!(
        "  loaded snapshot: {} features, {} vertices, {} MiB on the heap, {} MiB mapped ({:.2?})",
        map.features.len(),
        map.points.len(),
        heap >> 20,
        mapped >> 20,
        t.elapsed()
    );
    Ok(map)
}

/// A section of a mapped v2 file, bounds- and alignment-checked.
fn section<T: Pod>(bytes: &'static [u8], h: &Header, s: usize) -> Result<&'static [T]> {
    debug_assert_eq!(std::mem::size_of::<T>(), ELEMENT_BYTES[s]);
    let [offset, count] = h.sections[s];
    let len = (count as usize)
        .checked_mul(ELEMENT_BYTES[s])
        .context("section size overflows")?;
    let start = offset as usize;
    let end = start.checked_add(len).context("section end overflows")?;
    ensure!(
        start.is_multiple_of(ALIGN) && start >= HEADER_BYTES && end <= bytes.len(),
        "section {s} ({start}..{end}) is outside the file or misaligned"
    );
    bytemuck::try_cast_slice(&bytes[start..end]).map_err(|e| anyhow::anyhow!("section {s}: {e:?}"))
}

fn open_v2(bytes: &'static [u8]) -> Result<Map> {
    ensure!(bytes.len() >= HEADER_BYTES, "snapshot header is truncated");
    let h: Header = bytemuck::pod_read_unaligned(&bytes[..std::mem::size_of::<Header>()]);
    let points: &[[f32; 2]] = section(bytes, &h, POINTS)?;
    let ring_starts: &[u32] = section(bytes, &h, RING_STARTS)?;
    let features: &[Feature] = section(bytes, &h, FEATURES)?;
    let details: &[DetailRec] = section(bytes, &h, DETAILS)?;
    let elevations: &[f32] = section(bytes, &h, ELEVATIONS)?;
    let groups: &[[f32; 4]] = section(bytes, &h, GROUPS)?;
    let kind_counts: &[u64] = section(bytes, &h, KIND_COUNTS)?;
    let lens: &[u64] = section(bytes, &h, INDEX_LENS)?;
    let boxes: &[[f32; 4]] = section(bytes, &h, INDEX_BOXES)?;
    let ids: &[u32] = section(bytes, &h, INDEX_IDS)?;

    ensure!(
        ring_starts
            .last()
            .is_some_and(|&e| e as usize == points.len()),
        "ring starts do not end at the last point"
    );
    ensure!(
        kind_counts.len() <= Kind::COUNT,
        "snapshot has {} kinds, this build knows {}: rebuild or upgrade",
        kind_counts.len(),
        Kind::COUNT
    );
    ensure!(
        kind_counts.iter().sum::<u64>() == features.len() as u64,
        "kind counts do not add up to the feature count"
    );
    ensure!(
        lens.len() == MAX_BUCKET + 1,
        "index has {} buckets",
        lens.len()
    );
    let mut index = Vec::with_capacity(lens.len());
    let mut node = 0usize;
    for &len in lens {
        ensure!(len as usize <= features.len(), "index bucket is too large");
        let nodes = *index::levels(len as usize).last().unwrap() as usize;
        ensure!(node + nodes <= boxes.len(), "index is truncated");
        index.push(Packed::from_parts(
            len as usize,
            Store::Mapped(&boxes[node..node + nodes]),
            Store::Mapped(&ids[node..node + nodes]),
        )?);
        node += nodes;
    }
    ensure!(
        node == boxes.len() && node == ids.len(),
        "index has extra nodes"
    );
    Ok(Map {
        bounds: Rect::new(h.bounds[0], h.bounds[1], h.bounds[2], h.bounds[3]),
        origin: h.origin,
        background: if h.background == 1 {
            Background::Water
        } else {
            Background::Land
        },
        points: Store::Mapped(points),
        ring_starts: Store::Mapped(ring_starts),
        features: Store::Mapped(features),
        max_height: h.max_height,
        details: Store::Mapped(details),
        elevations: Store::Mapped(elevations),
        groups: Store::Mapped(groups),
        kind_counts: kind_counts.to_vec(),
        index,
    })
}

/// A v1 file's fields, its arrays as (unaligned) bytes. Features and details
/// have the same 56- and 24-byte records as v2.
struct V1<'a> {
    bounds: Rect,
    origin: [f64; 2],
    background: Background,
    max_height: f32,
    arrays: [&'a [u8]; 6],
}

/// Element sizes of v1's arrays, in file order: points, ring starts,
/// features, details, elevations, groups.
const V1_ELEMENT_BYTES: [usize; 6] = [8, 4, 56, 24, 4, 16];

fn parse_v1(b: &[u8]) -> Result<V1<'_>> {
    ensure!(b.len() >= 61 && &b[..8] == MAGIC_V1, "not a v1 snapshot");
    let f = |i: usize| f64::from_le_bytes(b[8 + 8 * i..16 + 8 * i].try_into().unwrap());
    let mut pos = 61;
    let mut arrays: [&[u8]; 6] = Default::default();
    for (a, size) in arrays.iter_mut().zip(V1_ELEMENT_BYTES) {
        let n = b
            .get(pos..pos + 8)
            .map(|n| u64::from_le_bytes(n.try_into().unwrap()) as usize)
            .context("v1 snapshot is truncated")?;
        let len = n.checked_mul(size).context("v1 array size overflows")?;
        *a = b
            .get(pos + 8..pos + 8 + len)
            .context("v1 snapshot is truncated")?;
        pos += 8 + len;
    }
    Ok(V1 {
        bounds: Rect::new(f(0), f(1), f(2), f(3)),
        origin: [f(4), f(5)],
        background: if b[56] == 1 {
            Background::Water
        } else {
            Background::Land
        },
        max_height: f32::from_le_bytes(b[57..61].try_into().unwrap()),
        arrays,
    })
}

impl V1<'_> {
    fn features(&self) -> impl Iterator<Item = Feature> + '_ {
        self.arrays[2]
            .as_chunks::<56>()
            .0
            .iter()
            .map(|r| bytemuck::pod_read_unaligned(r))
    }

    /// Every kind and roof byte is one this build knows.
    fn validate(&self) -> Result<()> {
        if let Some(f) = self
            .features()
            .find(|f| Kind::from_u8(f.raw_kind()).is_none())
        {
            bail!("unknown feature kind {}", f.raw_kind());
        }
        for rec in self.arrays[3].as_chunks::<24>().0 {
            let d: DetailRec = bytemuck::pod_read_unaligned(rec);
            ensure!(
                RoofShape::from_u8(d.raw_roof()).is_some(),
                "unknown roof shape {}",
                d.raw_roof()
            );
        }
        Ok(())
    }
}

/// Reads a v1 snapshot onto the heap and rebuilds its index.
fn load_v1(bytes: &[u8]) -> Result<Map> {
    let v1 = parse_v1(bytes)?;
    v1.validate()?;
    let [points, ring_starts, features, details, elevations, groups] = v1.arrays;
    Ok(Map::from_parts(
        v1.bounds,
        v1.origin,
        v1.background,
        bytemuck::pod_collect_to_vec(points),
        bytemuck::pod_collect_to_vec(ring_starts),
        bytemuck::pod_collect_to_vec(features),
        v1.max_height,
        bytemuck::pod_collect_to_vec(details),
        bytemuck::pod_collect_to_vec(elevations),
        bytemuck::pod_collect_to_vec(groups),
    ))
}

/// Converts a v1 snapshot to v2 without rebuilding the map: the arrays are
/// copied straight from the mapped input, and only the index and the kind
/// counts are computed (about 45 bytes per feature of memory, briefly).
pub fn convert(input: &Path, output: &Path) -> Result<()> {
    let t = Instant::now();
    let bytes = map_file(input)?;
    if bytes.get(..8) == Some(MAGIC) {
        bail!("{} is already a v2 snapshot", input.display());
    }
    let v1 = parse_v1(bytes).with_context(|| format!("reading {}", input.display()))?;
    v1.validate()?;
    let index = build_index(v1.features());
    let kind_counts = count_kinds(v1.features());
    let (lens, boxes, ids) = index_sections(&index);
    let a = v1.arrays;
    let parts = Parts {
        bounds: v1.bounds,
        origin: v1.origin,
        background: v1.background,
        max_height: v1.max_height,
        sections: [
            vec![a[0]],
            vec![a[1]],
            vec![a[2]],
            vec![a[3]],
            vec![a[4]],
            vec![a[5]],
            vec![bytemuck::cast_slice(&kind_counts)],
            vec![bytemuck::cast_slice(&lens)],
            boxes,
            ids,
        ],
    };
    write(output, &parts)?;
    eprintln!(
        "converted {} to {} ({} features) in {:.2?}",
        input.display(),
        output.display(),
        a[2].len() / 56,
        t.elapsed()
    );
    Ok(())
}

/// Writes `map` in the v1 format, for tests of reading and converting it.
#[cfg(test)]
pub(crate) fn save_v1(map: &Map, path: &Path) -> Result<()> {
    let mut w = BufWriter::new(File::create(path)?);
    w.write_all(MAGIC_V1)?;
    let b = &map.bounds;
    for v in [
        b.min_x,
        b.min_y,
        b.max_x,
        b.max_y,
        map.origin[0],
        map.origin[1],
    ] {
        w.write_all(&v.to_le_bytes())?;
    }
    w.write_all(&[(map.background == Background::Water) as u8])?;
    w.write_all(&map.max_height.to_le_bytes())?;
    let arrays: [&[u8]; 6] = [
        bytemuck::cast_slice(&map.points[..]),
        bytemuck::cast_slice(&map.ring_starts[..]),
        bytemuck::cast_slice(&map.features[..]),
        bytemuck::cast_slice(&map.details[..]),
        bytemuck::cast_slice(&map.elevations[..]),
        bytemuck::cast_slice(&map.groups[..]),
    ];
    for (a, size) in arrays.iter().zip(V1_ELEMENT_BYTES) {
        w.write_all(&((a.len() / size) as u64).to_le_bytes())?;
        w.write_all(a)?;
    }
    w.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests;
