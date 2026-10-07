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
//! Little-endian, 4-byte aligned, gzipped:
//!
//! ```text
//! "GTL1"  u32 background rgba
//! fills:     u32 vertices, u32 indices, [f32 u, f32 v, u32 rgba]*,              u32*
//! lines:     u32 vertices, u32 indices, [f32 u, v, z, nx, ny, dist, width, minpx,
//!                                        u32 rgba, f32 dash_on, dash_off]*,      u32*
//! meshes:    u32 vertices, u32 indices, [f32 u, v, z, nx, ny, nz, u32 rgba,
//!                                        f32 floor, base, seed]*,               u32*
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

fn rgba(c: Color) -> u32 {
    let c = c.to_color_u8();
    u32::from_le_bytes([c.red(), c.green(), c.blue(), c.alpha()])
}

#[derive(Default)]
struct Buffers {
    fill_v: Vec<u32>,
    fill_i: Vec<u32>,
    casing_v: Vec<u32>,
    casing_i: Vec<u32>,
    line_v: Vec<u32>,
    line_i: Vec<u32>,
    mesh_v: Vec<u32>,
    mesh_i: Vec<u32>,
    inst: Vec<u32>,
    instances: u32,
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
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write;
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut enc = GzEncoder::new(Vec::with_capacity(bytes.len() / 3), Compression::fast());
    enc.write_all(&bytes).expect("writing to memory");
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
                    solid(&mut b, map, theme, &tf, feat, id);
                }
            }
            Group::TransportTunnels => {}
            _ => line_feature(&mut b, map, theme, &tf, &ctx, feat, &tile_rect, tolerance),
        }
    }

    let mut out: Vec<u32> = Vec::with_capacity(
        8 + b.fill_v.len() + b.fill_i.len() + b.casing_v.len() + b.line_v.len() + b.mesh_v.len(),
    );
    out.push(u32::from_le_bytes(*b"GTL1"));
    out.push(rgba(bg));
    out.push((b.fill_v.len() / 3) as u32);
    out.push(b.fill_i.len() as u32);
    out.extend(&b.fill_v);
    out.extend(&b.fill_i);
    // Casings first, then fills, as one line section.
    const LINE: usize = 11;
    let casing_count = (b.casing_v.len() / LINE) as u32;
    out.push(((b.casing_v.len() + b.line_v.len()) / LINE) as u32);
    out.push((b.casing_i.len() + b.line_i.len()) as u32);
    out.extend(&b.casing_v);
    out.extend(&b.line_v);
    out.extend(&b.casing_i);
    out.extend(b.line_i.iter().map(|i| i + casing_count));
    out.push((b.mesh_v.len() / 10) as u32);
    out.push(b.mesh_i.len() as u32);
    out.extend(&b.mesh_v);
    out.extend(&b.mesh_i);
    out.push(b.instances);
    out.extend(&b.inst);
    gzip(&out)
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
        let base = (b.fill_v.len() / 3) as u32;
        for p in flat.as_chunks::<2>().0 {
            b.fill_v.extend([f(p[0]), f(p[1]), color]);
        }
        b.fill_i.extend(tris.iter().map(|&i| base + i as u32));
    }
}

/// Appends a ribbon for a polyline with per-vertex heights (meters).
#[allow(clippy::too_many_arguments)]
fn ribbon(
    verts: &mut Vec<u32>,
    idx: &mut Vec<u32>,
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
    let base = (verts.len() / 11) as u32;
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
            verts.extend([
                f(p[0]),
                f(p[1]),
                f(p[2]),
                f(nx * side),
                f(ny * side),
                f(dist),
                f(width_m),
                f(0.75),
                color,
                f(on),
                f(off),
            ]);
        }
        if i > 0 {
            let k = base + (i as u32 - 1) * 2;
            idx.extend([k, k + 1, k + 2, k + 1, k + 3, k + 2]);
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
            if raised_kind {
                curtain(b, tf, &top, feat.base as f64, theme.barrier_face(feat.kind));
            }
            if matches!(feat.kind, Kind::PowerLine | Kind::Aerialway | Kind::Gantry) {
                poles(b, tf, &top, theme.wire(feat.kind));
            }
            let bridge = deck_height(feat.kind, feat.layer, feat.flags) > 0.0
                || matches!(feat.kind, Kind::JetBridge | Kind::Pipeline);
            if top.iter().any(|p| p[2] > 0.3) && feat.kind.is_transport() || bridge {
                support(b, theme, tf, &top, &style, ctx.ppm, bridge, dist);
            }
            if let Some(c) = &style.casing {
                ribbon(&mut b.casing_v, &mut b.casing_i, &top, tf, c, ctx.ppm, dist);
            }
            if let Some(l) = &style.line {
                ribbon(&mut b.line_v, &mut b.line_i, &top, tf, l, ctx.ppm, dist);
            }
        }
    }
}

/// Pushes a triangle list (meters around `anchor_uv`) into the mesh buffer.
fn push_mesh(b: &mut Buffers, tf: &TileFrame, anchor: Point, tris: &[Vertex], window: [f64; 3]) {
    let base = (b.mesh_v.len() / 10) as u32;
    for v in tris {
        let u = anchor[0] + tf.units(v.pos[0]);
        let w = anchor[1] + tf.units(v.pos[1]);
        b.mesh_v.extend([
            f(u),
            f(w),
            f(v.pos[2]),
            f(v.normal[0]),
            f(v.normal[1]),
            f(v.normal[2]),
            rgba(v.color),
            f(window[0]),
            f(window[1]),
            f(window[2]),
        ]);
    }
    b.mesh_i.extend((0..tris.len() as u32).map(|i| base + i));
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

/// A building or other solid, if its anchor lies in this tile: walls from
/// base to eaves (window data for the shader) and its roof.
fn solid(b: &mut Buffers, map: &Map, theme: &Theme, tf: &TileFrame, feat: &Feature, id: u32) {
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
    let roof = roof_plan(feat, &detail, &footprint, base, top);
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

    let mut walls = Vec::new();
    let mut roof_tris = Vec::new();
    let mut flat_rings: Vec<Vec<[f64; 2]>> = Vec::new();
    for ring in map.rings(feat) {
        let pts: Vec<[f64; 2]> = ring.iter().map(|&p| to_m(p)).collect();
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
