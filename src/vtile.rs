//! Geometry tiles for the WebGL client: styled, triangulated meshes per XYZ tile.
//!
//! - fills: ground areas, clipped and triangulated
//! - lines: ribbons with width in meters, a pixel minimum, dashes and per-vertex height
//! - meshes: buildings, roofs, bridge sides, pillars, walls, poles
//! - instances: trees and street objects, drawn from shared templates
//!
//! Positions are tile units (0..1) horizontally, meters vertically. Solids
//! belong to the tile holding their anchor and are never cut.
//!
//! Little-endian, gzipped. Sections are `u32 vertices, u32 indices`, then
//! one column per vertex field (each padded to 4 bytes, so gzip sees like
//! with like), then indices: u16 (padded) below 65536 vertices, else u32.
//! Positions are i16 in 1/8192 tile and heights i16 in 1/20 m. Columns, in
//! order:
//!
//! ```text
//! "GTL3"  u32 background rgba
//! fills:  i16 u, v | u32 rgba
//! lines:  i16 u, v, z | u16 width_cm | f32 dist | u32 rgba |
//!         u16 dash_on_dm, dash_off_dm | i8 nx, ny (x64)
//! meshes: i16 u, v, z | u16 base_dm | u32 rgba | u16 seed | u8 floor_dm
//! prisms: u32 solids, u32 corners, u32 roof indices, then columns per solid
//!         u16 corners | u16 roof indices | u16 base_dm | u16 top_dm |
//!         u32 wall rgba | u32 roof rgba | u16 seed | u8 floor_dm,
//!         then i16 u, v per corner and u8 roof indices (per solid, padded
//!         once at the end). Flat-roofed solids come this way; the client
//!         extrudes walls between base and top.
//! instances: u32 count, [f32 u, v, z, heading, scale; u32 template]*
//! ```

use crate::classify::{flags, Group, Kind, TowerType};
use crate::elevation::{deck_height, METERS_PER_LAYER};
use crate::geo::{clip_ring, meters_per_unit, point_in_ring, signed_area2, Point, Rect, TILE_SIZE};
use crate::map::{Feature, Map};
use crate::render::mesh::{face_triangles, Mesh, Vertex};
use crate::render::models;
use crate::render::solids::roof_plan;
use crate::style::{Ctx, StrokeSpec, Theme};
use tiny_skia::Color;

/// Highest zoom with its own tiles; deeper views reuse them.
pub const MAX_ZOOM: u8 = 16;
/// Lowest zoom whose tiles carry 3D solids.
const SOLIDS_ZOOM: u8 = 15;
/// Distance between trees in a tree row, in meters.
const TREE_SPACING: f64 = 7.0;
/// Distance between bridge pillars, in meters.
const PILLAR_SPACING: f64 = 35.0;
/// Bumped with the encoding so cached tiles of another format are never reused.
pub const FORMAT: u32 = 3;
/// Quantization: tile units and meters per step.
const UV_STEPS: f64 = 8192.0;
const Z_STEPS: f64 = 20.0;
/// Vertex columns as `(offset, len)` in each record; padding is not sent.
const FILL_FIELDS: &[(usize, usize)] = &[(0, 4), (4, 4)];
const LINE_FIELDS: &[(usize, usize)] = &[(0, 6), (6, 2), (8, 4), (12, 4), (16, 4), (20, 2)];
const MESH_FIELDS: &[(usize, usize)] = &[(0, 6), (6, 2), (8, 4), (12, 2), (14, 1)];
const PRISM_FIELDS: &[(usize, usize)] = &[
    (0, 2),
    (2, 2),
    (4, 2),
    (6, 2),
    (8, 4),
    (12, 4),
    (16, 2),
    (18, 1),
];

fn rgba(c: Color) -> u32 {
    let c = c.to_color_u8();
    u32::from_le_bytes([c.red(), c.green(), c.blue(), c.alpha()])
}

/// Packed vertices and their indices.
#[derive(Default)]
struct Section {
    v: Vec<u8>,
    i: Vec<u32>,
    count: u32,
}

impl Section {
    fn vertex(&mut self, bytes: &[u8]) -> u32 {
        self.v.extend_from_slice(bytes);
        self.count += 1;
        self.count - 1
    }

    /// Writes `count, indices`, the vertices as one padded column per
    /// `(offset, len)` field of a `stride`-byte record, then the indices.
    fn write(&self, out: &mut Vec<u8>, stride: usize, fields: &[(usize, usize)]) {
        out.extend(self.count.to_le_bytes());
        out.extend((self.i.len() as u32).to_le_bytes());
        for &(at, len) in fields {
            for rec in self.v.chunks_exact(stride) {
                out.extend_from_slice(&rec[at..at + len]);
            }
            pad(out);
        }
        if self.count < 65536 {
            out.extend(self.i.iter().flat_map(|&i| (i as u16).to_le_bytes()));
        } else {
            out.extend(self.i.iter().flat_map(|&i| i.to_le_bytes()));
        }
        pad(out);
    }
}

fn pad(out: &mut Vec<u8>) {
    out.resize(out.len().next_multiple_of(4), 0);
}

#[derive(Default)]
struct Buffers {
    fill: Section,
    casing: Section,
    line: Section,
    mesh: Section,
    /// Mesh vertex bytes to index, so shared corners are stored once.
    mesh_seen: rustc_hash::FxHashMap<[u8; 16], u32>,
    /// Flat-roofed solids: per-solid records, outline corners, roof indices.
    prisms: Section,
    prism_corners: Vec<u8>,
    prism_roofs: Vec<u8>,
    inst: Vec<u32>,
    instances: u32,
}

fn qi16(v: f64, steps: f64) -> [u8; 2] {
    ((v * steps).round().clamp(-32768.0, 32767.0) as i16).to_le_bytes()
}

fn qu16(v: f64, steps: f64) -> [u8; 2] {
    ((v * steps).round().clamp(0.0, 65535.0) as u16).to_le_bytes()
}

fn qi8(v: f64, steps: f64) -> u8 {
    (v * steps).round().clamp(-127.0, 127.0) as i8 as u8
}

fn f(v: f64) -> u32 {
    (v as f32).to_bits()
}

/// Template index of a point object's model: one per kind, plus tower types
/// and conifers.
pub fn template_id(kind: Kind, variant: u8, conifer: bool) -> u32 {
    let base = (kind as u32 - Kind::Tree as u32) * 8;
    match kind {
        Kind::Tree if conifer => base + 1,
        Kind::Tower => base + variant as u32,
        _ => base,
    }
}

/// All templates: `(id, kind, variant, conifer)`.
fn template_list() -> Vec<(u32, Kind, u8, bool)> {
    let mut out = Vec::new();
    for k in (Kind::Tree as u8)..=(Kind::NavLight as u8) {
        // SAFETY-free conversion: kinds are a contiguous repr(u8) range.
        let kind = kind_from_u8(k);
        out.push((template_id(kind, 0, false), kind, 0, false));
        if kind == Kind::Tree {
            out.push((template_id(kind, 0, true), kind, 0, true));
        }
        if kind == Kind::Tower {
            for v in 1..8 {
                out.push((template_id(kind, v, false), kind, v, false));
            }
        }
    }
    out
}

fn kind_from_u8(k: u8) -> Kind {
    // Point kinds run from Tree to NavLight; walk the enum to stay safe.
    use Kind::*;
    const POINTS: [Kind; 34] = [
        Tree,
        Shrub,
        Stone,
        StreetLamp,
        TrafficSignal,
        StopSign,
        PowerPole,
        PowerTower,
        Flagpole,
        Mast,
        Chimney,
        Tower,
        WaterTower,
        Crane,
        Bollard,
        Block,
        Hydrant,
        Bench,
        PicnicTable,
        WasteBasket,
        PostBox,
        BicycleParking,
        DrinkingWater,
        Phone,
        SubwayEntrance,
        Shelter,
        Monument,
        Artwork,
        PlayEquipment,
        RailSignal,
        BufferStop,
        Cabinet,
        Windsock,
        NavLight,
    ];
    POINTS[(k - Tree as u8) as usize]
}

/// Typical height a template is built at; instances scale to their own.
fn template_height(kind: Kind, variant: u8) -> f64 {
    let t = crate::classify::Tags::default();
    if kind == Kind::Tower {
        // Mirror the tower types' typical heights.
        return match TowerType::from_u8(variant) {
            TowerType::Lighting => 20.0,
            TowerType::Monopole => 30.0,
            TowerType::Lattice | TowerType::Guyed => 45.0,
            TowerType::Belfry => 30.0,
            TowerType::Minaret => 35.0,
            _ => 25.0,
        };
    }
    kind.default_height(&t).max(0.3) as f64
}

/// The model templates for a theme, encoded:
/// `"GTM1" u32 count, then per template: u32 id, f32 height, u32 vertices,
/// [f32 x, y, z, nx, ny, nz, u32 rgba]*` (meters, x east, y south, z up,
/// facing south).
pub fn templates(theme: &Theme) -> Vec<u8> {
    let mut out: Vec<u32> = Vec::new();
    let list = template_list();
    out.push(u32::from_le_bytes(*b"GTM1"));
    out.push(list.len() as u32);
    let mut tris = Vec::new();
    for (id, kind, variant, conifer) in list {
        let h = template_height(kind, variant);
        let c = theme.object(kind);
        let mut m = Mesh::new();
        if kind == Kind::Tree {
            models::tree(&mut m, h, conifer, &c, theme.trunk());
        } else {
            m = models::model(kind, variant, h, &c, theme.trunk());
        }
        tris.clear();
        m.triangles(&mut tris);
        out.push(id);
        out.push(f(h));
        out.push(tris.len() as u32);
        for v in &tris {
            out.extend([f(v.pos[0]), f(v.pos[1]), f(v.pos[2])]);
            out.extend([f(v.normal[0]), f(v.normal[1]), f(v.normal[2])]);
            out.push(rgba(v.color));
        }
    }
    gzip(&out)
}

fn gzip(words: &[u32]) -> Vec<u8> {
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    gzip_bytes(&bytes)
}

fn gzip_bytes(bytes: &[u8]) -> Vec<u8> {
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write;
    let mut enc = GzEncoder::new(Vec::with_capacity(bytes.len() / 3), Compression::fast());
    enc.write_all(bytes).expect("writing to memory");
    enc.finish().expect("writing to memory")
}

/// Tile coordinate frame.
struct TileFrame {
    /// Tile origin and size in normalized world units.
    x0: f64,
    y0: f64,
    size: f64,
    /// Map origin (local coordinates are world minus this).
    origin: Point,
    /// Meters per tile unit (tile-normalized).
    m_per_unit: f64,
    /// Whether walls, supports and poles are built (too small below).
    solids: bool,
}

impl TileFrame {
    fn uv(&self, p: [f32; 2]) -> Point {
        [
            (p[0] as f64 + self.origin[0] - self.x0) / self.size,
            (p[1] as f64 + self.origin[1] - self.y0) / self.size,
        ]
    }

    /// Meters → tile units.
    fn units(&self, m: f64) -> f64 {
        m / self.m_per_unit
    }
}

/// Builds the geometry tile `(z, x, y)` in `theme`.
pub fn build(map: &Map, theme: &Theme, z: u8, x: u32, y: u32) -> Vec<u8> {
    let n = (1u64 << z) as f64;
    let size = 1.0 / n;
    let (x0, y0) = (x as f64 * size, y as f64 * size);
    let center_y = y0 + size / 2.0;
    let mpu = meters_per_unit(center_y.clamp(0.0, 1.0));
    let tf = TileFrame {
        x0,
        y0,
        size,
        origin: map.origin,
        m_per_unit: mpu * size,
        solids: z >= SOLIDS_ZOOM,
    };
    // Style as for the middle of the zoom range this tile serves.
    let style_zoom = z as f32 + 0.5;
    let ppm = (TILE_SIZE * (style_zoom as f64).exp2() / mpu) as f32;
    let ctx = Ctx {
        zoom: style_zoom,
        scale: 1.0,
        ppm,
    };
    let vis = if z >= MAX_ZOOM { 30.0 } else { z as f32 + 0.5 };

    let margin = size * 0.02;
    let area = [
        (x0 - margin - map.origin[0]) as f32,
        (y0 - margin - map.origin[1]) as f32,
        (x0 + size + margin - map.origin[0]) as f32,
        (y0 + size + margin - map.origin[1]) as f32,
    ];
    let mut ids = Vec::new();
    map.query(area, vis, &mut ids);

    let mut b = Buffers::default();
    let bg = match map.background {
        crate::assemble::Background::Land => theme.land_color(),
        crate::assemble::Background::Water => theme.water_color(),
    };
    let tile_rect = Rect::new(-0.01, -0.01, 1.01, 1.01);
    // Simplify lower zooms: half a pixel of a 512 px tile.
    let tolerance = if z >= MAX_ZOOM { 0.0 } else { 0.5 / 512.0 };

    for &id in &ids {
        let feat = &map.features[id as usize];
        let group = feat.group();
        match group {
            Group::Land | Group::Areas | Group::AreaOverlays => {
                if let Some(style) = theme.area(feat.kind, &ctx) {
                    fill(
                        &mut b,
                        map,
                        &tf,
                        feat,
                        rgba(style.fill),
                        &tile_rect,
                        tolerance,
                    );
                }
            }
            // Level of detail: solids from z15, objects at full detail only.
            Group::Objects => {
                if feat.kind.is_point() {
                    if z >= MAX_ZOOM {
                        instance(&mut b, map, &tf, feat);
                    }
                } else if z >= SOLIDS_ZOOM {
                    solid(&mut b, map, theme, &tf, feat, id, None);
                } else if keep_at(feat, z, mpu) {
                    solid(&mut b, map, theme, &tf, feat, id, Some(1.0 / 256.0));
                }
            }
            Group::TransportTunnels => {}
            _ => line_feature(&mut b, map, theme, &tf, &ctx, feat, &tile_rect, tolerance),
        }
    }

    let mut out = Vec::with_capacity(
        64 + b.fill.v.len() + b.casing.v.len() + b.line.v.len() + b.mesh.v.len(),
    );
    out.extend(b"GTL3");
    out.extend(rgba(bg).to_le_bytes());
    b.fill.write(&mut out, 8, FILL_FIELDS);
    // Casings first, then lines, as one section.
    let mut lines = b.casing;
    let first = lines.count;
    lines.v.extend(&b.line.v);
    lines.i.extend(b.line.i.iter().map(|i| i + first));
    lines.count += b.line.count;
    lines.write(&mut out, 24, LINE_FIELDS);
    b.mesh.write(&mut out, 16, MESH_FIELDS);
    out.extend(b.prisms.count.to_le_bytes());
    out.extend(((b.prism_corners.len() / 4) as u32).to_le_bytes());
    out.extend((b.prism_roofs.len() as u32).to_le_bytes());
    for &(at, len) in PRISM_FIELDS {
        for rec in b.prisms.v.as_chunks::<20>().0 {
            out.extend_from_slice(&rec[at..at + len]);
        }
        pad(&mut out);
    }
    out.extend(&b.prism_corners);
    out.extend(&b.prism_roofs);
    pad(&mut out);
    out.extend(b.instances.to_le_bytes());
    out.extend(b.inst.iter().flat_map(|w| w.to_le_bytes()));
    gzip_bytes(&out)
}

/// Ramer-Douglas-Peucker simplification on tile units (keeps endpoints).
fn simplify(pts: &[Point], tol: f64) -> Vec<Point> {
    if tol <= 0.0 || pts.len() <= 2 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0, pts.len() - 1)];
    while let Some((a, z)) = stack.pop() {
        let (pa, pz) = (pts[a], pts[z]);
        let (dx, dy) = (pz[0] - pa[0], pz[1] - pa[1]);
        let len = (dx * dx + dy * dy).sqrt().max(1e-12);
        let (mut best, mut at) = (0.0, 0);
        for (i, p) in pts.iter().enumerate().take(z).skip(a + 1) {
            let d = ((p[0] - pa[0]) * dy - (p[1] - pa[1]) * dx).abs() / len;
            if d > best {
                best = d;
                at = i;
            }
        }
        if best > tol {
            keep[at] = true;
            stack.push((a, at));
            stack.push((at, z));
        }
    }
    pts.iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(p, _)| *p)
        .collect()
}

fn fill(
    b: &mut Buffers,
    map: &Map,
    tf: &TileFrame,
    feat: &Feature,
    color: u32,
    rect: &Rect,
    tol: f64,
) {
    let mut rings: Vec<Vec<Point>> = Vec::new();
    let mut tmp = Vec::new();
    for ring in map.rings(feat) {
        let mut pts: Vec<Point> = ring.iter().map(|&p| tf.uv(p)).collect();
        pts = simplify(&pts, tol);
        clip_ring(&mut pts, rect, &mut tmp);
        if pts.len() >= 3 && signed_area2(&pts).abs() > 1e-12 {
            rings.push(pts);
        }
    }
    if rings.is_empty() {
        return;
    }
    // Outer rings wind like the largest ring; holes the other way.
    let largest = rings
        .iter()
        .max_by(|a, c| signed_area2(a).abs().total_cmp(&signed_area2(c).abs()))
        .map(|r| signed_area2(r) > 0.0)
        .unwrap_or(true);
    let (outers, holes): (Vec<&Vec<Point>>, Vec<&Vec<Point>>) = rings
        .iter()
        .partition(|r| (signed_area2(r) > 0.0) == largest);
    for outer in outers {
        let mine: Vec<&Vec<Point>> = holes
            .iter()
            .filter(|h| point_in_ring(h[0], outer))
            .copied()
            .collect();
        let mut flat: Vec<f64> = outer.iter().flat_map(|p| [p[0], p[1]]).collect();
        let mut hole_idx = Vec::new();
        for h in &mine {
            hole_idx.push(flat.len() / 2);
            flat.extend(h.iter().flat_map(|p| [p[0], p[1]]));
        }
        let Ok(tris) = earcutr::earcut(&flat, &hole_idx, 2) else {
            continue;
        };
        let base = b.fill.count;
        for p in flat.as_chunks::<2>().0 {
            let [u0, u1] = qi16(p[0], UV_STEPS);
            let [v0, v1] = qi16(p[1], UV_STEPS);
            let c = color.to_le_bytes();
            b.fill.vertex(&[u0, u1, v0, v1, c[0], c[1], c[2], c[3]]);
        }
        b.fill.i.extend(tris.iter().map(|&i| base + i as u32));
    }
}

/// Appends a ribbon for a polyline with per-vertex heights (meters).
#[allow(clippy::too_many_arguments)]
fn ribbon(
    sec: &mut Section,
    pts: &[[f64; 3]],
    tf: &TileFrame,
    spec: &StrokeSpec,
    ppm: f32,
    start_dist: f64,
) {
    if pts.len() < 2 {
        return;
    }
    let width_m = spec.width as f64 / ppm as f64;
    let (on, off) = spec.dash.map_or((0.0, 0.0), |[a, b]| {
        (a as f64 / ppm as f64, b as f64 / ppm as f64)
    });
    let color = rgba(spec.color);
    let n = pts.len();
    let dir = |a: [f64; 3], b: [f64; 3]| {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = (dx * dx + dy * dy).sqrt().max(1e-12);
        [dx / l, dy / l]
    };
    let mut dist = start_dist;
    let base = sec.count;
    for i in 0..n {
        let d_in = if i > 0 {
            Some(dir(pts[i - 1], pts[i]))
        } else {
            None
        };
        let d_out = if i + 1 < n {
            Some(dir(pts[i], pts[i + 1]))
        } else {
            None
        };
        let normal = |d: [f64; 2]| [-d[1], d[0]];
        let (nx, ny) = match (d_in, d_out) {
            (Some(a), Some(c)) => {
                let (na, nc) = (normal(a), normal(c));
                let m = [na[0] + nc[0], na[1] + nc[1]];
                let ml = (m[0] * m[0] + m[1] * m[1]).sqrt();
                if ml < 1e-6 {
                    (na[0], na[1])
                } else {
                    // Miter, limited so sharp turns do not spike.
                    let m = [m[0] / ml, m[1] / ml];
                    let k = (1.0 / (m[0] * na[0] + m[1] * na[1]).max(0.5)).min(2.0);
                    (m[0] * k, m[1] * k)
                }
            }
            (Some(a), None) | (None, Some(a)) => {
                let na = normal(a);
                (na[0], na[1])
            }
            (None, None) => (0.0, 0.0),
        };
        if i > 0 {
            let (a, c) = (pts[i - 1], pts[i]);
            dist += ((c[0] - a[0]).powi(2) + (c[1] - a[1]).powi(2)).sqrt() * tf.m_per_unit;
        }
        let p = pts[i];
        for side in [1.0, -1.0] {
            let mut v = [0u8; 24];
            v[0..2].copy_from_slice(&qi16(p[0], UV_STEPS));
            v[2..4].copy_from_slice(&qi16(p[1], UV_STEPS));
            v[4..6].copy_from_slice(&qi16(p[2], Z_STEPS));
            v[6..8].copy_from_slice(&qu16(width_m, 100.0));
            v[8..12].copy_from_slice(&(dist as f32).to_le_bytes());
            v[12..16].copy_from_slice(&color.to_le_bytes());
            v[16..18].copy_from_slice(&qu16(on, 10.0));
            v[18..20].copy_from_slice(&qu16(off, 10.0));
            v[20] = qi8(nx * side, 64.0);
            v[21] = qi8(ny * side, 64.0);
            sec.vertex(&v);
        }
        if i > 0 {
            let k = base + (i as u32 - 1) * 2;
            sec.i.extend([k, k + 1, k + 2, k + 1, k + 3, k + 2]);
        }
    }
}

/// Clips a polyline with heights to the tile rectangle (Liang-Barsky),
/// interpolating heights; returns pieces with their start distance (meters).
fn clip3(pts: &[[f64; 3]], r: &Rect, m_per_unit: f64) -> Vec<(Vec<[f64; 3]>, f64)> {
    let mut out: Vec<(Vec<[f64; 3]>, f64)> = Vec::new();
    let (mut open, mut dist) = (false, 0.0);
    let lerp = |a: [f64; 3], b: [f64; 3], t: f64| {
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    };
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let d = [b[0] - a[0], b[1] - a[1]];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt() * m_per_unit;
        let (mut t0, mut t1, mut ok) = (0.0_f64, 1.0_f64, true);
        for (p, q) in [
            (-d[0], a[0] - r.min_x),
            (d[0], r.max_x - a[0]),
            (-d[1], a[1] - r.min_y),
            (d[1], r.max_y - a[1]),
        ] {
            if p == 0.0 {
                ok &= q >= 0.0;
            } else if p < 0.0 {
                t0 = t0.max(q / p);
            } else {
                t1 = t1.min(q / p);
            }
        }
        if ok && t0 <= t1 {
            if !open || t0 > 0.0 {
                out.push((vec![lerp(a, b, t0)], dist + t0 * len));
                open = true;
            }
            if let Some(last) = out.last_mut() {
                last.0.push(lerp(a, b, t1));
            }
            if t1 < 1.0 {
                open = false;
            }
        } else {
            open = false;
        }
        dist += len;
    }
    out.retain(|(p, _)| p.len() >= 2);
    out
}

#[allow(clippy::too_many_arguments)]
fn line_feature(
    b: &mut Buffers,
    map: &Map,
    theme: &Theme,
    tf: &TileFrame,
    ctx: &Ctx,
    feat: &Feature,
    rect: &Rect,
    tol: f64,
) {
    let style = theme.line(feat.kind, feat.flags, feat.height, ctx);
    let elev = map.elevations(feat);
    let group = feat.group();
    // Heights: elevation profiles, else a fixed height for raised kinds.
    let fixed = match feat.kind {
        Kind::Hedge | Kind::Fence | Kind::Wall | Kind::LowBarrier | Kind::Dam | Kind::Weir => {
            Some(feat.height as f64)
        }
        Kind::PowerLine | Kind::Aerialway | Kind::Gantry | Kind::JetBridge | Kind::Pipeline => {
            Some(feat.height as f64)
        }
        _ if group == Group::TransportBridges => {
            Some(feat.layer.max(1) as f64 * METERS_PER_LAYER as f64)
        }
        _ => None,
    };
    let mut offset = 0;
    for ring in map.rings(feat) {
        let n = ring.len();
        let heights: Vec<f64> = match elev {
            Some(e) => e[offset..offset + n].iter().map(|&h| h as f64).collect(),
            None => vec![0.0; n],
        };
        offset += n;
        let pts: Vec<[f64; 3]> = ring
            .iter()
            .zip(&heights)
            .map(|(&p, &h)| {
                let uv = tf.uv(p);
                [uv[0], uv[1], h]
            })
            .collect();
        let pts = if tol > 0.0 && elev.is_none() {
            let flat: Vec<Point> = pts.iter().map(|p| [p[0], p[1]]).collect();
            simplify(&flat, tol)
                .into_iter()
                .map(|p| [p[0], p[1], 0.0])
                .collect()
        } else {
            pts
        };
        for (piece, dist) in clip3(&pts, rect, tf.m_per_unit) {
            let raised_kind = matches!(
                feat.kind,
                Kind::Hedge | Kind::Fence | Kind::Wall | Kind::LowBarrier | Kind::Dam | Kind::Weir
            );
            if feat.kind == Kind::TreeRow {
                tree_row(b, tf, feat, &piece, dist);
                continue;
            }
            // The line itself, at its height.
            let top: Vec<[f64; 3]> = match fixed {
                Some(h) if elev.is_none() => piece.iter().map(|p| [p[0], p[1], h]).collect(),
                _ => piece.clone(),
            };
            if raised_kind && tf.solids {
                curtain(b, tf, &top, feat.base as f64, theme.barrier_face(feat.kind));
            }
            if tf.solids && matches!(feat.kind, Kind::PowerLine | Kind::Aerialway | Kind::Gantry) {
                poles(b, tf, &top, theme.wire(feat.kind));
            }
            let bridge = deck_height(feat.kind, feat.layer, feat.flags) > 0.0
                || matches!(feat.kind, Kind::JetBridge | Kind::Pipeline);
            if tf.solids && (top.iter().any(|p| p[2] > 0.3) && feat.kind.is_transport() || bridge) {
                support(b, theme, tf, &top, &style, ctx.ppm, bridge, dist);
            }
            if let Some(c) = &style.casing {
                ribbon(&mut b.casing, &top, tf, c, ctx.ppm, dist);
            }
            if let Some(l) = &style.line {
                ribbon(&mut b.line, &top, tf, l, ctx.ppm, dist);
            }
        }
    }
}

/// Pushes a triangle list (meters around `anchor_uv`) into the mesh buffer.
/// Normals are not sent (the shader derives flat ones), so faces share
/// their corners and identical vertices are stored once.
fn push_mesh(b: &mut Buffers, tf: &TileFrame, anchor: Point, tris: &[Vertex], window: [f64; 3]) {
    let [floor, base, seed] = window;
    for t in tris {
        let mut v = [0u8; 16];
        v[0..2].copy_from_slice(&qi16(anchor[0] + tf.units(t.pos[0]), UV_STEPS));
        v[2..4].copy_from_slice(&qi16(anchor[1] + tf.units(t.pos[1]), UV_STEPS));
        v[4..6].copy_from_slice(&qi16(t.pos[2], Z_STEPS));
        v[6..8].copy_from_slice(&qu16(base, 10.0));
        v[8..12].copy_from_slice(&rgba(t.color).to_le_bytes());
        v[12..14].copy_from_slice(&(seed as u16).to_le_bytes());
        v[14] = (floor * 10.0).round().clamp(0.0, 255.0) as u8;
        let mesh = &mut b.mesh;
        let i = *b.mesh_seen.entry(v).or_insert_with(|| mesh.vertex(&v));
        b.mesh.i.push(i);
    }
}

/// Vertical faces between `base` meters and a line's heights.
fn curtain(b: &mut Buffers, tf: &TileFrame, top: &[[f64; 3]], base: f64, color: Option<Color>) {
    let Some(color) = color else { return };
    let anchor = [top[0][0], top[0][1]];
    let m = |p: [f64; 3], z: f64| {
        [
            (p[0] - anchor[0]) * tf.m_per_unit,
            (p[1] - anchor[1]) * tf.m_per_unit,
            z,
        ]
    };
    let mut tris = Vec::new();
    for w in top.windows(2) {
        let (a, c) = (w[0], w[1]);
        let quad = [
            m(a, base.min(a[2])),
            m(c, base.min(c[2])),
            m(c, c[2]),
            m(a, a[2]),
        ];
        face_triangles(&quad, color, &mut tris);
    }
    push_mesh(b, tf, anchor, &tris, [0.0; 3]);
}

/// Poles at a wire's vertices.
fn poles(b: &mut Buffers, tf: &TileFrame, top: &[[f64; 3]], color: Color) {
    let anchor = [top[0][0], top[0][1]];
    let mut mesh = Mesh::new();
    for p in [top[0], top[top.len() - 1]] {
        let e = (p[0] - anchor[0]) * tf.m_per_unit;
        let s = (p[1] - anchor[1]) * tf.m_per_unit;
        mesh.beam([e, s, 0.0], [e, s, p[2]], 0.6, color);
    }
    let mut tris = Vec::new();
    mesh.triangles(&mut tris);
    push_mesh(b, tf, anchor, &tris, [0.0; 3]);
}

/// What holds up a raised road or rail: a deck with pillars and a visible
/// side for bridges, an embankment for ramps.
#[allow(clippy::too_many_arguments)]
fn support(
    b: &mut Buffers,
    theme: &Theme,
    tf: &TileFrame,
    top: &[[f64; 3]],
    style: &crate::style::LineStyle,
    ppm: f32,
    bridge: bool,
    dist: f64,
) {
    let anchor = [top[0][0], top[0][1]];
    let half = style
        .casing
        .as_ref()
        .or(style.line.as_ref())
        .map_or(1.0, |s| s.width as f64 / ppm as f64)
        / 2.0;
    let m = |p: [f64; 3]| {
        [
            (p[0] - anchor[0]) * tf.m_per_unit,
            (p[1] - anchor[1]) * tf.m_per_unit,
            p[2],
        ]
    };
    let mut tris = Vec::new();
    let mut mesh = Mesh::new();
    let side = if bridge {
        theme.deck_side()
    } else {
        theme.bank()
    };
    let mut run = dist;
    let mut next_pillar = (dist / PILLAR_SPACING).ceil() * PILLAR_SPACING;
    for w in top.windows(2) {
        let (a, c) = (m(w[0]), m(w[1]));
        let (dx, dy) = (c[0] - a[0], c[1] - a[1]);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-6 {
            continue;
        }
        let nrm = [-dy / len * half, dx / len * half];
        for s in [1.0, -1.0] {
            let (ea, ec) = (
                [a[0] + nrm[0] * s, a[1] + nrm[1] * s],
                [c[0] + nrm[0] * s, c[1] + nrm[1] * s],
            );
            let (za, zc) = if bridge {
                ((a[2] - 1.5).max(0.0), (c[2] - 1.5).max(0.0))
            } else {
                (0.0, 0.0)
            };
            let quad = [
                [ea[0], ea[1], za],
                [ec[0], ec[1], zc],
                [ec[0], ec[1], c[2]],
                [ea[0], ea[1], a[2]],
            ];
            face_triangles(&quad, side, &mut tris);
        }
        if bridge {
            // Deck underside.
            let under = [
                [a[0] + nrm[0], a[1] + nrm[1], (a[2] - 1.5).max(0.0)],
                [c[0] + nrm[0], c[1] + nrm[1], (c[2] - 1.5).max(0.0)],
                [c[0] - nrm[0], c[1] - nrm[1], (c[2] - 1.5).max(0.0)],
                [a[0] - nrm[0], a[1] - nrm[1], (a[2] - 1.5).max(0.0)],
            ];
            face_triangles(&under, side, &mut tris);
            while next_pillar <= run + len {
                let t = (next_pillar - run) / len;
                let p = [a[0] + dx * t, a[1] + dy * t, a[2] + (c[2] - a[2]) * t];
                if p[2] > 2.0 {
                    mesh.beam([p[0], p[1], 0.0], [p[0], p[1], p[2] - 1.5], 1.5, side);
                }
                next_pillar += PILLAR_SPACING;
            }
        }
        run += len;
    }
    mesh.triangles(&mut tris);
    push_mesh(b, tf, anchor, &tris, [0.0; 3]);
}

fn tree_row(b: &mut Buffers, tf: &TileFrame, feat: &Feature, pts: &[[f64; 3]], dist: f64) {
    let mut run = dist;
    let mut next = (dist / TREE_SPACING).ceil() * TREE_SPACING;
    for w in pts.windows(2) {
        let (a, c) = (w[0], w[1]);
        let len = ((c[0] - a[0]).powi(2) + (c[1] - a[1]).powi(2)).sqrt() * tf.m_per_unit;
        while len > 0.0 && next <= run + len {
            let t = (next - run) / len;
            let p = [a[0] + (c[0] - a[0]) * t, a[1] + (c[1] - a[1]) * t];
            if (0.0..1.0).contains(&p[0]) && (0.0..1.0).contains(&p[1]) {
                let h = feat.height.max(2.0) as f64;
                let tpl = template_id(Kind::Tree, 0, false);
                let scale = h / template_height(Kind::Tree, 0);
                b.inst.extend([
                    f(p[0]),
                    f(p[1]),
                    f(0.0),
                    f(std::f64::consts::PI),
                    f(scale),
                    tpl,
                ]);
                b.instances += 1;
            }
            next += TREE_SPACING;
        }
        run += len;
    }
}

/// A point object as an instance, if its anchor lies in this tile.
fn instance(b: &mut Buffers, map: &Map, tf: &TileFrame, feat: &Feature) {
    let p = tf.uv(map.ring(feat.ring_start)[0]);
    if !(0.0..1.0).contains(&p[0]) || !(0.0..1.0).contains(&p[1]) {
        return;
    }
    let conifer = feat.flags & flags::CONIFER != 0;
    let tpl = template_id(feat.kind, feat.variant, conifer);
    let h = (feat.height - feat.base).max(0.1) as f64;
    let scale = h / template_height(feat.kind, feat.variant);
    let heading = if feat.heading.is_finite() {
        feat.heading as f64
    } else {
        std::f64::consts::PI
    };
    b.inst.extend([
        f(p[0]),
        f(p[1]),
        f(feat.base as f64),
        f(heading),
        f(scale),
        tpl,
    ]);
    b.instances += 1;
}

/// A flat-roofed solid as its outline (meters around `anchor`), heights,
/// wall and roof colors, window data and roof triangles.
#[allow(clippy::too_many_arguments)]
fn prism(
    b: &mut Buffers,
    tf: &TileFrame,
    anchor: Point,
    ring: &[[f64; 2]],
    tris: &[usize],
    [base, top]: [f32; 2],
    [wall, roof]: [u32; 2],
    [floor, _, seed]: [f64; 3],
) {
    let mut rec = [0u8; 20];
    rec[0..2].copy_from_slice(&(ring.len() as u16).to_le_bytes());
    rec[2..4].copy_from_slice(&(tris.len() as u16).to_le_bytes());
    rec[4..6].copy_from_slice(&qu16(base as f64, 10.0));
    rec[6..8].copy_from_slice(&qu16(top as f64, 10.0));
    rec[8..12].copy_from_slice(&wall.to_le_bytes());
    rec[12..16].copy_from_slice(&roof.to_le_bytes());
    rec[16..18].copy_from_slice(&(seed as u16).to_le_bytes());
    rec[18] = (floor * 10.0).round().clamp(0.0, 255.0) as u8;
    b.prisms.vertex(&rec);
    for p in ring {
        b.prism_corners
            .extend(qi16(anchor[0] + tf.units(p[0]), UV_STEPS));
        b.prism_corners
            .extend(qi16(anchor[1] + tf.units(p[1]), UV_STEPS));
    }
    b.prism_roofs.extend(tris.iter().map(|&i| i as u8));
}

/// Whether a solid is worth drawing in a low-detail tile: everything at
/// z14, then only the larger or taller, down to the skyline.
fn keep_at(feat: &Feature, z: u8, mpu: f64) -> bool {
    let [x0, y0, x1, y1] = feat.bbox;
    let area = (x1 - x0) as f64 * (y1 - y0) as f64 * mpu * mpu;
    let h = feat.height;
    match z {
        14.. => true,
        13 => h >= 20.0 || area >= 600.0,
        12 => h >= 40.0 || area >= 5000.0,
        11 => h >= 80.0,
        10 => h >= 150.0,
        _ => false,
    }
}

/// A building or other solid, if its anchor lies in this tile: walls from
/// base to eaves (window data for the shader) and its roof. With `lod` (a
/// simplification tolerance in tile units) it is a flat-roofed prism of its
/// simplified outline, for distant tiles.
fn solid(
    b: &mut Buffers,
    map: &Map,
    theme: &Theme,
    tf: &TileFrame,
    feat: &Feature,
    id: u32,
    lod: Option<f64>,
) {
    let outer = map.ring(feat.ring_start);
    let n = outer.len() as f64;
    let (sx, sy) = outer
        .iter()
        .fold((0.0, 0.0), |(x, y), p| (x + p[0] as f64, y + p[1] as f64));
    let anchor_local = [(sx / n) as f32, (sy / n) as f32];
    let anchor = tf.uv(anchor_local);
    if !(0.0..1.0).contains(&anchor[0]) || !(0.0..1.0).contains(&anchor[1]) {
        return;
    }
    let detail = map.detail(feat);
    let (mut base, top) = (feat.base, feat.height);
    if feat.kind == Kind::Canopy {
        base = base.max(top - 0.4);
    }
    let to_m = |p: [f32; 2]| {
        let uv = tf.uv(p);
        [
            (uv[0] - anchor[0]) * tf.m_per_unit,
            (uv[1] - anchor[1]) * tf.m_per_unit,
        ]
    };
    let footprint: Vec<[f64; 2]> = outer.iter().map(|&p| to_m(p)).collect();
    let roof = match lod {
        Some(_) => None,
        None => roof_plan(feat, &detail, &footprint, base, top),
    };
    let eave = roof.as_ref().map_or(top, |r| r.eave);
    let facade = match detail.facade_colour {
        Some(c) => theme.mapped_facade(c, 1.0),
        None => theme.facade(feat.kind, 1.0),
    };
    let roof_color = detail
        .roof_colour
        .map(|c| theme.mapped_colour(c))
        .unwrap_or_else(|| {
            if roof.is_some() {
                theme.pitched_roof()
            } else {
                theme.roof(feat.kind)
            }
        });
    let floors = if detail.levels >= 1.0 {
        detail.levels as f64
    } else {
        ((eave - base) as f64 / 3.2).round().max(1.0)
    };
    let window = if feat.kind == Kind::Building && eave - base >= 2.5 {
        [
            (eave - base) as f64 / floors,
            base as f64,
            (id % 9973) as f64,
        ]
    } else {
        [0.0, 0.0, 0.0]
    };

    let rings: Vec<Vec<[f64; 2]>> = match lod {
        Some(tol) => vec![lod_outline(&footprint, tol * tf.m_per_unit)],
        None => map
            .rings(feat)
            .map(|r| r.iter().map(|&p| to_m(p)).collect())
            .collect(),
    };
    // Flat roof over one outline: send the outline, the client extrudes it.
    if roof.is_none() && rings.len() == 1 {
        let mut ring = rings[0].clone();
        if ring.len() > 1 && ring.first() == ring.last() {
            ring.pop();
        }
        if (3..=255).contains(&ring.len()) {
            let flat: Vec<f64> = ring.iter().flat_map(|p| [p[0], p[1]]).collect();
            if let Ok(tris) = earcutr::earcut(&flat, &[], 2) {
                let colors = [rgba(facade), rgba(roof_color)];
                prism(b, tf, anchor, &ring, &tris, [base, top], colors, window);
                return;
            }
        }
    }
    let mut walls = Vec::new();
    let mut roof_tris = Vec::new();
    let mut flat_rings: Vec<Vec<[f64; 2]>> = Vec::new();
    for pts in rings {
        let k = pts.len();
        if k < 3 {
            continue;
        }
        if roof
            .as_ref()
            .is_none_or(|r| !matches!(r_shape(r), crate::classify::RoofShape::Skillion))
        {
            for i in 0..k {
                let (a, c) = (pts[i], pts[(i + 1) % k]);
                let quad = [
                    [a[0], a[1], base as f64],
                    [c[0], c[1], base as f64],
                    [c[0], c[1], eave as f64],
                    [a[0], a[1], eave as f64],
                ];
                face_triangles(&quad, facade, &mut walls);
            }
        }
        flat_rings.push(pts);
    }
    match &roof {
        Some(plan) => {
            let mut mesh = Mesh::new();
            plan.build(&mut mesh, &footprint, base, roof_color, facade);
            mesh.triangles(&mut roof_tris);
        }
        None => {
            // A flat roof over all rings (holes as courtyards).
            let mut flat: Vec<f64> = Vec::new();
            let mut holes = Vec::new();
            for (i, r) in flat_rings.iter().enumerate() {
                if i > 0 {
                    holes.push(flat.len() / 2);
                }
                flat.extend(r.iter().flat_map(|p| [p[0], p[1]]));
            }
            if let Ok(t) = earcutr::earcut(&flat, &holes, 2) {
                let pt = |i: usize| [flat[i * 2], flat[i * 2 + 1], top as f64];
                for tri in t.as_chunks::<3>().0 {
                    for &i in tri {
                        roof_tris.push(Vertex {
                            pos: pt(i),
                            normal: [0.0, 0.0, 1.0],
                            color: roof_color,
                        });
                    }
                }
            }
        }
    }
    push_mesh(b, tf, anchor, &walls, window);
    push_mesh(b, tf, anchor, &roof_tris, [0.0; 3]);
}

/// A footprint simplified to `tol` meters; tiny ones become their box.
fn lod_outline(pts: &[[f64; 2]], tol: f64) -> Vec<[f64; 2]> {
    let mut ring = simplify(pts, tol);
    if ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }
    if ring.len() >= 3 {
        return ring;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in pts {
        (x0, y0, x1, y1) = (x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1]));
    }
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

fn r_shape(r: &crate::render::solids::RoofPlan) -> crate::classify::RoofShape {
    r.shape()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_cover_every_point_kind() {
        let ids: Vec<u32> = template_list().iter().map(|t| t.0).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "template ids are unique");
        assert_eq!(kind_from_u8(Kind::NavLight as u8), Kind::NavLight);
        assert_eq!(kind_from_u8(Kind::Hydrant as u8), Kind::Hydrant);
    }

    #[test]
    fn simplify_keeps_corners() {
        let pts = [[0.0, 0.0], [0.5, 0.001], [1.0, 0.0], [1.0, 1.0]];
        let s = simplify(&pts, 0.01);
        assert_eq!(s, vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]);
    }
}
