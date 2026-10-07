//! Map snapshots: a built [`Map`] saved to disk, so a server loads it in
//! seconds with no more memory than serving needs (building from an
//! `.osm.pbf` peaks at over twice that).
//!
//! Layout, little-endian: `"MAPSNAP1"`, then a header (bounds, origin,
//! background, max height) and length-prefixed arrays of points, ring
//! starts, features, details, elevations and groups. The spatial index is
//! rebuilt on load.

use crate::assemble::Background;
use crate::classify::{Detail, Kind, RoofShape};
use crate::geo::Rect;
use crate::map::{Feature, Map};
use anyhow::{bail, ensure, Context, Result};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::time::Instant;

#[cfg(target_endian = "big")]
compile_error!("snapshots are written as little-endian memory");

pub const MAGIC: &[u8; 8] = b"MAPSNAP1";
const FEATURE_BYTES: usize = 56;
const DETAIL_BYTES: usize = 24;

/// Whether `path` holds a snapshot rather than an `.osm.pbf`.
pub fn is_snapshot(path: &Path) -> bool {
    let mut head = [0u8; 8];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|_| &head == MAGIC)
}

/// Plain numeric arrays as raw bytes.
trait Pod: Copy + Default {}
impl Pod for u32 {}
impl Pod for f32 {}
impl Pod for [f32; 2] {}
impl Pod for [f32; 4] {}

fn write_array<T: Pod>(w: &mut impl Write, v: &[T]) -> Result<()> {
    w.write_all(&(v.len() as u64).to_le_bytes())?;
    // SAFETY: T is plain numbers with no padding; the target is little-endian.
    let bytes =
        unsafe { std::slice::from_raw_parts(v.as_ptr().cast::<u8>(), std::mem::size_of_val(v)) };
    w.write_all(bytes)?;
    Ok(())
}

fn read_len(r: &mut impl Read) -> Result<usize> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b) as usize)
}

fn read_array<T: Pod>(r: &mut impl Read) -> Result<Vec<T>> {
    let n = read_len(r)?;
    let mut v = vec![T::default(); n];
    // SAFETY: as in `write_array`; every bit pattern is a valid T.
    let bytes = unsafe {
        std::slice::from_raw_parts_mut(v.as_mut_ptr().cast::<u8>(), std::mem::size_of_val(&v[..]))
    };
    r.read_exact(bytes)?;
    Ok(v)
}

fn kind_from_u8(v: u8) -> Result<Kind> {
    ensure!(v <= Kind::NavLight as u8, "unknown feature kind {v}");
    // SAFETY: Kind is repr(u8) with contiguous discriminants 0..=NavLight.
    Ok(unsafe { std::mem::transmute::<u8, Kind>(v) })
}

fn roof_from_u8(v: u8) -> Result<RoofShape> {
    Ok(match v {
        0 => RoofShape::Flat,
        1 => RoofShape::Gabled,
        2 => RoofShape::Hipped,
        3 => RoofShape::Pyramidal,
        4 => RoofShape::Skillion,
        5 => RoofShape::Dome,
        _ => bail!("unknown roof shape {v}"),
    })
}

fn put_feature(out: &mut Vec<u8>, f: &Feature) {
    out.extend([f.kind as u8, f.flags, f.layer as u8, f.variant]);
    for v in [f.ring_start, f.ring_count, f.detail, f.elev, f.group] {
        out.extend(v.to_le_bytes());
    }
    for v in [f.height, f.base, f.heading, f.vis_zoom] {
        out.extend(v.to_le_bytes());
    }
    for v in f.bbox {
        out.extend(v.to_le_bytes());
    }
}

fn get_feature(b: &[u8]) -> Result<Feature> {
    let u = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
    let fl = |i: usize| f32::from_le_bytes(b[i..i + 4].try_into().unwrap());
    Ok(Feature {
        kind: kind_from_u8(b[0])?,
        flags: b[1],
        layer: b[2] as i8,
        variant: b[3],
        ring_start: u(4),
        ring_count: u(8),
        detail: u(12),
        elev: u(16),
        group: u(20),
        height: fl(24),
        base: fl(28),
        heading: fl(32),
        vis_zoom: fl(36),
        bbox: [fl(40), fl(44), fl(48), fl(52)],
    })
}

fn put_detail(out: &mut Vec<u8>, d: &Detail) {
    let present = d.roof_colour.is_some() as u8 | (d.facade_colour.is_some() as u8) << 1;
    out.extend([d.roof as u8, d.across as u8, present, 0]);
    out.extend(d.roof_height.to_le_bytes());
    out.extend(d.levels.to_le_bytes());
    out.extend(d.roof_colour.unwrap_or(0).to_le_bytes());
    out.extend(d.facade_colour.unwrap_or(0).to_le_bytes());
    out.extend([0u8; 4]);
}

fn get_detail(b: &[u8]) -> Result<Detail> {
    let u = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
    let fl = |i: usize| f32::from_le_bytes(b[i..i + 4].try_into().unwrap());
    Ok(Detail {
        roof: roof_from_u8(b[0])?,
        across: b[1] != 0,
        roof_colour: (b[2] & 1 != 0).then(|| u(12)),
        facade_colour: (b[2] & 2 != 0).then(|| u(16)),
        roof_height: fl(4),
        levels: fl(8),
    })
}

/// Writes records of `size` bytes, a chunk at a time.
fn write_records<T>(
    w: &mut impl Write,
    items: &[T],
    size: usize,
    put: impl Fn(&mut Vec<u8>, &T),
) -> Result<()> {
    w.write_all(&(items.len() as u64).to_le_bytes())?;
    let mut buf = Vec::with_capacity(size * 65536);
    for chunk in items.chunks(65536) {
        buf.clear();
        for item in chunk {
            put(&mut buf, item);
        }
        w.write_all(&buf)?;
    }
    Ok(())
}

fn read_records<T>(
    r: &mut impl Read,
    size: usize,
    get: impl Fn(&[u8]) -> Result<T>,
) -> Result<Vec<T>> {
    let n = read_len(r)?;
    let mut out = Vec::with_capacity(n);
    let mut buf = vec![0u8; size * 65536];
    let mut left = n;
    while left > 0 {
        let k = left.min(65536);
        r.read_exact(&mut buf[..k * size])?;
        for rec in buf[..k * size].chunks_exact(size) {
            out.push(get(rec)?);
        }
        left -= k;
    }
    Ok(out)
}

pub fn save(map: &Map, path: &Path) -> Result<()> {
    let t = Instant::now();
    let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut w = BufWriter::with_capacity(1 << 22, file);
    w.write_all(MAGIC)?;
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
    write_array(&mut w, &map.points)?;
    write_array(&mut w, &map.ring_starts)?;
    write_records(&mut w, &map.features, FEATURE_BYTES, put_feature)?;
    write_records(&mut w, &map.details, DETAIL_BYTES, put_detail)?;
    write_array(&mut w, &map.elevations)?;
    write_array(&mut w, &map.groups)?;
    w.flush()?;
    eprintln!("saved {} in {:.2?}", path.display(), t.elapsed());
    Ok(())
}

pub fn load(path: &Path) -> Result<Map> {
    let t = Instant::now();
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut r = BufReader::with_capacity(1 << 22, file);
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic)?;
    ensure!(&magic == MAGIC, "{} is not a map snapshot", path.display());
    let mut f64s = [0f64; 6];
    for v in &mut f64s {
        let mut b = [0u8; 8];
        r.read_exact(&mut b)?;
        *v = f64::from_le_bytes(b);
    }
    let mut one = [0u8; 1];
    r.read_exact(&mut one)?;
    let mut four = [0u8; 4];
    r.read_exact(&mut four)?;
    let points = read_array(&mut r)?;
    let ring_starts = read_array(&mut r)?;
    let features = read_records(&mut r, FEATURE_BYTES, get_feature)?;
    let details = read_records(&mut r, DETAIL_BYTES, get_detail)?;
    let elevations = read_array(&mut r)?;
    let groups = read_array(&mut r)?;
    let map = Map::from_parts(
        Rect::new(f64s[0], f64s[1], f64s[2], f64s[3]),
        [f64s[4], f64s[5]],
        if one[0] == 1 {
            Background::Water
        } else {
            Background::Land
        },
        points,
        ring_starts,
        features,
        f32::from_le_bytes(four),
        details,
        elevations,
        groups,
    );
    eprintln!(
        "  loaded snapshot: {} features, {} vertices ({:.2?})",
        map.features.len(),
        map.points.len(),
        t.elapsed()
    );
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_round_trip() {
        let f = Feature {
            kind: Kind::Building,
            flags: 3,
            layer: -2,
            ring_start: 7,
            ring_count: 2,
            height: 120.5,
            base: 4.0,
            detail: 9,
            variant: 1,
            heading: f32::NAN,
            elev: u32::MAX,
            group: 5,
            vis_zoom: 13.5,
            bbox: [0.1, 0.2, 0.3, 0.4],
        };
        let mut b = Vec::new();
        put_feature(&mut b, &f);
        assert_eq!(b.len(), FEATURE_BYTES);
        let g = get_feature(&b).unwrap();
        assert_eq!(
            (g.kind, g.layer, g.ring_start, g.elev),
            (f.kind, f.layer, 7, u32::MAX)
        );
        assert!(g.heading.is_nan() && g.bbox == f.bbox && g.height == f.height);
    }

    #[test]
    fn details_round_trip() {
        let d = Detail {
            roof: RoofShape::Hipped,
            roof_height: 3.5,
            across: true,
            roof_colour: Some(0x8a4b38),
            facade_colour: None,
            levels: 12.0,
        };
        let mut b = Vec::new();
        put_detail(&mut b, &d);
        assert_eq!(b.len(), DETAIL_BYTES);
        let e = get_detail(&b).unwrap();
        assert_eq!(e, d);
    }

    #[test]
    fn every_kind_reads_back() {
        for v in 0..=Kind::NavLight as u8 {
            assert_eq!(kind_from_u8(v).unwrap() as u8, v);
        }
        assert!(kind_from_u8(Kind::NavLight as u8 + 1).is_err());
    }
}
