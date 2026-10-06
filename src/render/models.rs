//! Bespoke 3D models for point objects, built at real-world dimensions.
//!
//! Each physical kind has its own model made of [`Mesh`] primitives in meters
//! around its map position (`[east, south, up]`): a bench has legs, a seat
//! and a backrest; a hydrant a barrel, a cap and outlets; a pylon a braced
//! lattice with crossarms. Models are scaled by the camera like everything
//! else, so proportions stay true at every zoom; detail simply becomes
//! visible as it grows past a pixel.

use super::camera::Frame;
use super::mesh::{shade, Mesh, V3};
use super::paint::paint;
use super::Renderer;
use crate::classify::{flags, Kind, TowerType};
use crate::geo::Point;
use crate::map::Feature;
use crate::style::ObjectPaint;
use tiny_skia::{Color, FillRule, PathBuilder, Pixmap, Rect, Transform};

fn rgb(c: u32) -> Color {
    Color::from_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, 255)
}

fn rgba(c: u32, a: f32) -> Color {
    Color::from_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, (a * 255.0) as u8)
}

/// A face visible from both sides (flags, sign plates, glass panels).
fn panel(m: &mut Mesh, pts: Vec<V3>, color: Color) {
    let mut back = pts.clone();
    back.reverse();
    m.face(pts, color);
    m.face(back, color);
}

/// A vertical rectangle in the east–up plane at `s`, centered at `e`.
fn sign_plate(m: &mut Mesh, e: f64, s: f64, w: f64, z0: f64, z1: f64, color: Color) {
    let hw = w / 2.0;
    panel(
        m,
        vec![
            [e - hw, s, z0],
            [e + hw, s, z0],
            [e + hw, s, z1],
            [e - hw, s, z1],
        ],
        color,
    );
}

fn tree(m: &mut Mesh, h: f64, conifer: bool, c: &ObjectPaint, trunk: Color) {
    if conifer {
        m.cylinder(0.0, 0.0, 0.15, 0.0, 0.2 * h, trunk, trunk, 6);
        m.cone(0.0, 0.0, 0.24 * h, 0.15 * h, 0.75 * h, c.accent, 10);
        m.cone(0.0, 0.0, 0.17 * h, 0.45 * h, h, c.accent, 10);
        return;
    }
    let crown = (0.3 * h).max(1.2);
    m.cylinder(0.0, 0.0, 0.16 + 0.01 * h, 0.0, h - crown, trunk, trunk, 6);
    m.ball([0.0, 0.0, h - crown], crown, c.body, c.top);
}

/// A braced steel lattice: legs tapering from `base` to `top` half-widths,
/// rings and cross-bracing on the viewer-facing sides.
fn lattice(m: &mut Mesh, h: f64, base: f64, top: f64, w: f64, color: Color) {
    let corners = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
    let half = |z: f64| base + (top - base) * z / h;
    for [x, y] in corners {
        m.beam([x * base, y * base, 0.0], [x * top, y * top, h], w, color);
    }
    let levels = ((h / 6.0) as usize).clamp(2, 24);
    for i in 1..=levels {
        let (z0, z1) = (
            (i - 1) as f64 * h / levels as f64,
            i as f64 * h / levels as f64,
        );
        let (s0, s1) = (half(z0), half(z1));
        for k in 0..4 {
            let (p, q) = (corners[k], corners[(k + 1) % 4]);
            m.beam(
                [p[0] * s1, p[1] * s1, z1],
                [q[0] * s1, q[1] * s1, z1],
                w * 0.6,
                color,
            );
        }
        m.beam([-s0, s0, z0], [s1, s1, z1], w * 0.5, color);
        m.beam([s0, s0, z0], [-s1, s1, z1], w * 0.5, color);
        m.beam([s0, -s0, z0], [s1, s1, z1], w * 0.5, color);
    }
}

/// Bespoke models of `man_made=tower` by tower type.
fn tower(m: &mut Mesh, h: f64, variant: u8, c: &ObjectPaint) {
    let (body, top, accent) = (c.body, c.top, c.accent);
    let steel = rgb(0x8d949c);
    let antenna = rgb(0xd8d6d0);
    match TowerType::from_u8(variant) {
        TowerType::Monopole => {
            m.cylinder(0.0, 0.0, 0.45, 0.0, 0.5 * h, steel, steel, 8);
            m.cylinder(0.0, 0.0, 0.3, 0.5 * h, h, steel, steel, 8);
            // Three sectors of panel antennas near the top.
            for k in 0..3 {
                let t = k as f64 * std::f64::consts::TAU / 3.0 + 0.5;
                let (x, y) = (0.7 * t.cos(), 0.7 * t.sin());
                m.cuboid(x, y, 0.3, 0.3, h - 2.5, h - 0.3, antenna, antenna);
            }
        }
        TowerType::Lattice => lattice(m, h, (0.06 * h).clamp(1.0, 4.0), 0.5, 0.18, steel),
        TowerType::Guyed => {
            lattice(m, h, 0.5, 0.5, 0.12, steel);
            for k in 0..3 {
                let t = k as f64 * std::f64::consts::TAU / 3.0;
                let r = 0.45 * h;
                for frac in [0.5, 0.95] {
                    m.beam(
                        [0.0, 0.0, frac * h],
                        [r * t.cos(), r * t.sin(), 0.0],
                        0.03,
                        rgb(0x6a6e74),
                    );
                }
            }
            m.ball([0.0, 0.0, h], 0.4, rgb(0xe0442c), rgb(0xffffff));
        }
        TowerType::Lighting => {
            m.cylinder(0.0, 0.0, 0.3, 0.0, h, steel, steel, 8);
            // A frame of floodlights facing the field.
            m.cuboid(0.0, 0.3, 3.2, 0.25, h - 1.6, h, steel, steel);
            for row in 0..2 {
                for col in 0..4 {
                    let x = -1.2 + col as f64 * 0.8;
                    let z = h - 0.5 - row as f64 * 0.8;
                    m.ball([x, 0.5, z], 0.22, rgb(0xfff2c8), rgb(0xffffff));
                }
            }
        }
        TowerType::Belfry => {
            // A masonry shaft, an open belfry, and a spire.
            let w = (0.12 * h).clamp(3.0, 8.0);
            m.cuboid(0.0, 0.0, w, w, 0.0, 0.62 * h, body, top);
            for [x, y] in [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]] {
                m.cuboid(
                    x * w * 0.4,
                    y * w * 0.4,
                    w * 0.2,
                    w * 0.2,
                    0.62 * h,
                    0.74 * h,
                    body,
                    body,
                );
            }
            m.cuboid(0.0, 0.0, w * 1.05, w * 1.05, 0.74 * h, 0.77 * h, top, top);
            m.cone(0.0, 0.0, w * 0.65, 0.77 * h, h, accent, 4);
        }
        TowerType::Minaret => {
            let r = (0.05 * h).clamp(1.2, 3.0);
            m.cylinder(0.0, 0.0, r, 0.0, 0.7 * h, body, top, 12);
            m.cylinder(0.0, 0.0, r * 1.6, 0.7 * h, 0.73 * h, top, top, 12);
            m.cylinder(0.0, 0.0, r * 0.8, 0.73 * h, 0.85 * h, body, top, 12);
            m.cone(0.0, 0.0, r * 0.9, 0.85 * h, h, accent, 12);
        }
        TowerType::Observation | TowerType::Generic => {
            m.cylinder(0.0, 0.0, 3.0, 0.0, 0.85 * h, body, top, 12);
            m.cylinder(0.0, 0.0, 4.2, 0.85 * h, 0.92 * h, top, top, 12);
            m.beam([0.0, 0.0, 0.92 * h], [0.0, 0.0, h], 0.3, rgb(0x5d6168));
        }
    }
}

/// Builds the model of `kind`, `h` meters tall.
fn model(kind: Kind, variant: u8, h: f64, c: &ObjectPaint, theme_trunk: Color) -> Mesh {
    use Kind as K;
    let mut m = Mesh::new();
    let (body, top, accent) = (c.body, c.top, c.accent);
    let metal = rgb(0x5d6168);
    let dark = rgb(0x2a2d31);
    match kind {
        K::Tree => tree(&mut m, h, false, c, theme_trunk),
        K::Shrub => m.ball([0.0, 0.0, 0.7], 0.8, body, top),
        K::Stone => {
            let fp: Vec<[f64; 2]> = (0..6)
                .map(|i| {
                    let t = i as f64 * std::f64::consts::TAU / 6.0;
                    let r = if i % 2 == 0 { 0.55 } else { 0.42 };
                    [r * t.cos(), r * t.sin()]
                })
                .collect();
            m.prism(&fp, 0.0, h, body, top);
        }
        K::StreetLamp => {
            m.cylinder(0.0, 0.0, 0.09, 0.0, h, body, body, 6);
            m.beam([0.0, 0.0, h - 0.1], [0.0, 1.3, h], 0.08, body);
            m.cuboid(0.0, 1.45, 0.25, 0.5, h - 0.25, h - 0.05, body, body);
            m.ball([0.0, 1.45, h - 0.3], 0.22, accent, rgb(0xffffff));
        }
        K::TrafficSignal => {
            m.cylinder(0.0, 0.0, 0.1, 0.0, h, body, body, 6);
            m.cuboid(0.0, 0.18, 0.36, 0.3, h - 1.3, h - 0.2, top, top);
            for (z, col) in [
                (h - 0.45, 0xe0443a),
                (h - 0.75, 0xf0b33a),
                (h - 1.05, 0x4cc36a),
            ] {
                m.ball([0.0, 0.36, z], 0.11, rgb(col), rgb(0xffffff));
            }
        }
        K::StopSign => {
            m.cylinder(0.0, 0.0, 0.03, 0.0, h - 0.4, metal, metal, 4);
            let (r, zc) = (0.38, h - 0.38);
            let oct: Vec<V3> = (0..8)
                .map(|i| {
                    let t = (i as f64 + 0.5) * std::f64::consts::TAU / 8.0;
                    [r * t.cos(), 0.04, zc + r * t.sin()]
                })
                .collect();
            panel(&mut m, oct, accent);
        }
        K::PowerPole => {
            m.cylinder(0.0, 0.0, 0.14, 0.0, h, body, top, 6);
            m.cuboid(0.0, 0.0, 2.4, 0.12, h - 0.75, h - 0.6, body, body);
            for e in [-1.0, 0.0, 1.0] {
                m.cylinder(
                    e,
                    0.0,
                    0.06,
                    h - 0.6,
                    h - 0.35,
                    rgb(0xd8d4cc),
                    rgb(0xd8d4cc),
                    6,
                );
            }
        }
        K::PowerTower => {
            let (b, t) = (3.0, 0.8);
            let corners = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
            let w = 0.25;
            for [x, y] in corners {
                m.beam([x * b, y * b, 0.0], [x * t, y * t, 0.82 * h], w, body);
                m.beam([x * t, y * t, 0.82 * h], [0.0, 0.0, h], w, body);
            }
            for frac in [0.25, 0.5, 0.82] {
                let s = b + (t - b) * frac / 0.82;
                let z = frac * h;
                for i in 0..4 {
                    let (p, q) = (corners[i], corners[(i + 1) % 4]);
                    m.beam(
                        [p[0] * s, p[1] * s, z],
                        [q[0] * s, q[1] * s, z],
                        w * 0.7,
                        body,
                    );
                }
            }
            // Bracing on the viewer-facing sides.
            for (z0, z1) in [(0.0, 0.25), (0.25, 0.5), (0.5, 0.82)] {
                let (s0, s1) = (b + (t - b) * z0 / 0.82, b + (t - b) * z1 / 0.82);
                m.beam([-s0, s0, z0 * h], [s1, s1, z1 * h], w * 0.6, body);
                m.beam([s0, s0, z0 * h], [-s1, s1, z1 * h], w * 0.6, body);
            }
            for (z, arm) in [(0.78 * h, 9.0), (0.93 * h, 6.0)] {
                m.cuboid(0.0, 0.0, arm, 0.5, z - 0.5, z, body, body);
                for e in [-arm / 2.0, arm / 2.0] {
                    m.beam([e, 0.0, z - 0.5], [e, 0.0, z - 2.0], 0.15, rgb(0xd8d4cc));
                }
            }
        }
        K::Flagpole => {
            m.cylinder(0.0, 0.0, 0.06, 0.0, h, body, body, 6);
            m.ball([0.0, 0.0, h + 0.08], 0.1, rgb(0xc9a640), rgb(0xffffff));
            panel(
                &mut m,
                vec![
                    [0.05, 0.0, h - 1.2],
                    [1.6, 0.05, h - 1.15],
                    [1.6, 0.05, h - 0.2],
                    [0.05, 0.0, h - 0.2],
                ],
                accent,
            );
        }
        K::Mast => {
            m.cylinder(0.0, 0.0, 0.35, 0.0, h * 0.6, body, top, 4);
            m.cylinder(0.0, 0.0, 0.2, h * 0.6, h, body, top, 4);
            m.ball([0.0, 0.0, h], 0.3, accent, rgb(0xffffff));
        }
        K::Chimney => {
            m.cylinder(0.0, 0.0, 1.6, 0.0, h - 1.0, body, top, 12);
            m.cylinder(0.0, 0.0, 1.75, h - 1.0, h, shade(body, 0.8), dark, 12);
        }
        K::Tower => tower(&mut m, h, variant, c),
        K::WaterTower => {
            // New York's wooden rooftop tanks: legs, a staved barrel with
            // steel hoops, and a conical roof.
            let r = 2.5;
            let leg_top = 0.4 * h;
            for [x, y] in [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]] {
                m.beam(
                    [x * r * 0.75, y * r * 0.75, 0.0],
                    [x * r * 0.75, y * r * 0.75, leg_top],
                    0.2,
                    dark,
                );
            }
            m.cylinder(0.0, 0.0, r, leg_top, 0.86 * h, body, body, 14);
            for f in [0.5, 0.62, 0.74] {
                m.cylinder(0.0, 0.0, r + 0.03, f * h, f * h + 0.12, metal, metal, 14);
            }
            m.cone(0.0, 0.0, r + 0.15, 0.86 * h, h, top, 14);
        }
        K::Crane => {
            let s = 0.8;
            for [x, y] in [[-s, -s], [s, -s], [s, s], [-s, s]] {
                m.beam([x, y, 0.0], [x, y, h], 0.2, body);
            }
            for i in 1..(h / 4.0) as usize {
                let z = i as f64 * 4.0;
                m.beam([-s, s, z - 4.0], [s, s, z], 0.12, body);
            }
            m.cuboid(5.0, 0.0, 34.0, 1.2, h, h + 1.2, body, body);
            m.cuboid(
                -9.0,
                0.0,
                3.0,
                2.2,
                h - 1.5,
                h + 0.8,
                rgb(0xa8a196),
                rgb(0xc7c0b5),
            );
            m.cuboid(1.5, 0.0, 2.2, 2.0, h - 2.5, h, rgb(0xe8e6e0), rgb(0xffffff));
            m.beam([18.0, 0.0, h], [18.0, 0.0, h - 15.0], 0.06, dark);
        }
        K::Bollard => {
            m.cylinder(0.0, 0.0, 0.12, 0.0, h - 0.06, body, body, 8);
            m.ball([0.0, 0.0, h - 0.06], 0.12, top, top);
        }
        K::Block => m.cuboid(0.0, 0.0, 1.0, 1.0, 0.0, h, body, top),
        K::Hydrant => {
            m.cylinder(0.0, 0.0, 0.14, 0.0, 0.62, body, body, 8);
            m.cylinder(0.0, 0.0, 0.17, 0.0, 0.08, shade(body, 0.85), body, 8);
            m.cylinder(0.0, 0.0, 0.1, 0.62, 0.72, top, top, 8);
            m.ball([0.0, 0.0, 0.74], 0.07, top, top);
            m.cuboid(0.0, 0.18, 0.14, 0.12, 0.38, 0.52, top, top);
            m.cuboid(-0.18, 0.0, 0.1, 0.09, 0.42, 0.52, top, top);
            m.cuboid(0.18, 0.0, 0.1, 0.09, 0.42, 0.52, top, top);
        }
        K::Bench => {
            for [x, y] in [[-0.8, -0.15], [0.8, -0.15], [-0.8, 0.15], [0.8, 0.15]] {
                m.cuboid(x, y, 0.06, 0.06, 0.0, 0.42, metal, metal);
            }
            m.cuboid(0.0, 0.0, 1.8, 0.45, 0.42, 0.48, body, top);
            m.cuboid(0.0, -0.21, 1.8, 0.05, 0.5, 0.88, body, top);
        }
        K::PicnicTable => {
            m.cuboid(0.0, 0.0, 2.0, 0.8, 0.72, 0.77, body, top);
            for s in [-0.65, 0.65] {
                m.cuboid(0.0, s, 2.0, 0.3, 0.42, 0.46, body, top);
            }
            for e in [-0.8, 0.8] {
                m.beam([e, -0.75, 0.0], [e, 0.0, 0.74], 0.07, body);
                m.beam([e, 0.75, 0.0], [e, 0.0, 0.74], 0.07, body);
            }
        }
        K::WasteBasket => {
            m.cylinder(0.0, 0.0, 0.3, 0.0, h - 0.05, body, dark, 10);
            m.cylinder(0.0, 0.0, 0.32, h - 0.05, h, top, top, 10);
        }
        K::PostBox => {
            for [x, y] in [[-0.2, -0.2], [0.2, -0.2], [0.2, 0.2], [-0.2, 0.2]] {
                m.cuboid(x, y, 0.05, 0.05, 0.0, 0.25, dark, dark);
            }
            m.cuboid(0.0, 0.0, 0.5, 0.5, 0.25, 1.05, body, top);
            m.cylinder(0.0, 0.0, 0.25, 1.05, 1.2, top, top, 10);
        }
        K::BicycleParking => {
            for i in -1..=1 {
                let x = i as f64 * 0.8;
                m.beam([x, -0.35, 0.0], [x, -0.35, 0.8], 0.05, body);
                m.beam([x, -0.35, 0.8], [x, 0.35, 0.8], 0.05, body);
                m.beam([x, 0.35, 0.8], [x, 0.35, 0.0], 0.05, body);
            }
        }
        K::DrinkingWater => {
            m.cylinder(0.0, 0.0, 0.14, 0.0, 0.8, body, body, 8);
            m.cylinder(0.0, 0.0, 0.3, 0.8, 0.95, top, rgb(0x9fc7d8), 10);
        }
        K::Phone => {
            for [x, y] in [[-0.45, -0.45], [0.45, -0.45], [0.45, 0.45], [-0.45, 0.45]] {
                m.cuboid(x, y, 0.08, 0.08, 0.0, 2.1, body, body);
            }
            m.cuboid(0.0, 0.0, 1.0, 1.0, 2.1, 2.3, body, top);
            sign_plate(&mut m, 0.0, -0.45, 0.85, 0.2, 2.0, rgba(0x9fbfd0, 0.45));
        }
        K::SubwayEntrance => {
            // Green iron railings around the stair, with globe lamps.
            let rail = body;
            m.cuboid(0.0, -1.0, 3.0, 0.08, 0.0, 1.05, rail, rail);
            m.cuboid(-1.5, 0.0, 0.08, 2.0, 0.0, 1.05, rail, rail);
            m.cuboid(1.5, 0.0, 0.08, 2.0, 0.0, 1.05, rail, rail);
            m.face(
                vec![
                    [-1.45, -0.95, 0.02],
                    [1.45, -0.95, 0.02],
                    [1.45, 1.0, 0.02],
                    [-1.45, 1.0, 0.02],
                ],
                dark,
            );
            for e in [-1.5, 1.5] {
                m.cylinder(e, 1.0, 0.06, 0.0, 2.6, rail, rail, 6);
                m.ball([e, 1.0, 2.75], 0.2, accent, rgb(0xffffff));
            }
        }
        K::Shelter => {
            for [x, y] in [[-1.5, -0.7], [1.5, -0.7], [-1.5, 0.7], [1.5, 0.7]] {
                m.cuboid(x, y, 0.08, 0.08, 0.0, 2.5, metal, metal);
            }
            m.cuboid(0.0, 0.0, 3.2, 1.6, 2.5, 2.65, top, top);
            sign_plate(&mut m, 0.0, -0.7, 3.0, 0.3, 2.4, rgba(0xa8c4d4, 0.45));
            m.cuboid(0.0, -0.45, 2.6, 0.35, 0.42, 0.47, metal, metal);
        }
        K::Monument => {
            m.cuboid(0.0, 0.0, 1.8, 1.8, 0.0, 0.5, body, top);
            m.cuboid(0.0, 0.0, 1.2, 1.2, 0.5, 0.9, body, top);
            m.cuboid(0.0, 0.0, 0.55, 0.55, 0.9, h - 0.4, body, top);
            m.cone(0.0, 0.0, 0.4, h - 0.4, h, top, 4);
        }
        K::Artwork => {
            m.cuboid(0.0, 0.0, 1.0, 1.0, 0.0, 0.7, rgb(0xa8a196), rgb(0xc7c0b5));
            m.beam([0.0, 0.0, 0.7], [0.3, 0.2, h - 0.6], 0.25, body);
            m.ball([0.3, 0.2, h - 0.5], 0.5, body, top);
        }
        K::PlayEquipment => {
            // A swing set: A-frames, a top bar and two seats on chains.
            for e in [-1.5, 1.5] {
                m.beam([e, -0.8, 0.0], [e, 0.0, 2.4], 0.08, body);
                m.beam([e, 0.8, 0.0], [e, 0.0, 2.4], 0.08, body);
            }
            m.beam([-1.5, 0.0, 2.4], [1.5, 0.0, 2.4], 0.1, top);
            for e in [-0.6, 0.6] {
                m.beam([e - 0.2, 0.0, 2.4], [e - 0.2, 0.0, 0.5], 0.02, dark);
                m.beam([e + 0.2, 0.0, 2.4], [e + 0.2, 0.0, 0.5], 0.02, dark);
                m.cuboid(e, 0.0, 0.5, 0.2, 0.45, 0.5, accent, accent);
            }
        }
        K::RailSignal => {
            m.cylinder(0.0, 0.0, 0.08, 0.0, h, body, body, 6);
            m.cuboid(0.0, 0.15, 0.3, 0.25, h - 0.9, h, top, top);
            m.ball([0.0, 0.3, h - 0.25], 0.09, accent, rgb(0xffffff));
            m.ball([0.0, 0.3, h - 0.6], 0.09, rgb(0x4cc36a), rgb(0xffffff));
        }
        K::BufferStop => {
            for e in [-0.75, 0.75] {
                m.beam([e, 0.3, 0.0], [e, 0.0, 1.0], 0.12, metal);
                m.beam([e, -0.4, 0.0], [e, 0.0, 1.0], 0.12, metal);
            }
            m.cuboid(0.0, 0.0, 1.9, 0.3, 0.85, 1.2, body, top);
        }
        K::Cabinet => {
            m.cuboid(0.0, 0.0, 0.9, 0.45, 0.0, h - 0.05, body, top);
            m.cuboid(0.0, 0.0, 1.0, 0.55, h - 0.05, h, top, top);
        }
        K::Windsock => {
            m.cylinder(0.0, 0.0, 0.05, 0.0, h, body, body, 6);
            panel(
                &mut m,
                vec![
                    [0.0, 0.0, h - 0.5],
                    [1.6, 0.0, h - 0.35],
                    [1.6, 0.0, h - 0.15],
                    [0.0, 0.0, h],
                ],
                accent,
            );
        }
        _ => {
            m.lit = false;
            m.ball([0.0, 0.0, 0.2], 0.2, accent, accent);
        }
    }
    m
}

fn oval(pixmap: &mut Pixmap, x: f64, y: f64, r: f64, color: Color) {
    if let Some(rect) = Rect::from_ltrb(
        (x - r) as f32,
        (y - r) as f32,
        (x + r) as f32,
        (y + r) as f32,
    ) {
        let mut pb = PathBuilder::new();
        pb.push_oval(rect);
        if let Some(path) = pb.finish() {
            pixmap.fill_path(
                &path,
                &paint(color),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}

impl Renderer<'_> {
    /// A tree of a tree row, standing at ground point `at` (screen).
    pub(super) fn draw_tree(
        &self,
        pixmap: &mut Pixmap,
        fr: &Frame,
        at: Point,
        base: f32,
        height: f32,
        conifer: bool,
    ) {
        let c = self.theme.object(Kind::Tree);
        let mut m = Mesh::new();
        tree(
            &mut m,
            height.max(2.0) as f64,
            conifer,
            &c,
            self.theme.trunk(),
        );
        let at = [at[0], at[1] - base as f64 * fr.lift_per_m];
        m.draw(pixmap, fr, at);
    }

    /// A point object, drawn from its bespoke model.
    pub(super) fn draw_prop(&self, pixmap: &mut Pixmap, fr: &Frame, f: &Feature) {
        let p = self.map.ring(f.ring_start)[0];
        let ground = fr.apply(p[0] as f64, p[1] as f64);
        let h = (f.height - f.base).max(0.1) as f64;
        // Skip objects that cannot reach the view.
        let reach = h * fr.lift_per_m + 40.0 * fr.ctx.scale as f64;
        let view = fr.rect(reach);
        if !view.contains(ground) && !(ground[1] > view.max_y && ground[1] - reach < fr.height) {
            return;
        }
        let at = [ground[0], ground[1] - f.base as f64 * fr.lift_per_m];
        let c = self.theme.object(f.kind);
        let mut m = model(f.kind, f.variant, h, &c, self.theme.trunk());
        if f.kind == Kind::Tree && f.flags & flags::CONIFER != 0 {
            m = Mesh::new();
            tree(&mut m, h, true, &c, self.theme.trunk());
        }
        m.draw(pixmap, fr, at);
    }

    /// Point objects seen straight down: crowns and small footprints.
    pub(super) fn draw_props_flat(&self, pixmap: &mut Pixmap, fr: &Frame, ids: &[u32]) {
        let ppm = fr.ctx.ppm as f64;
        let view = fr.rect(8.0);
        for &id in ids {
            let f = &self.map.features[id as usize];
            let p = self.map.ring(f.ring_start)[0];
            let [x, y] = fr.apply(p[0] as f64, p[1] as f64);
            if !view.contains([x, y]) {
                continue;
            }
            let c = self.theme.object(f.kind);
            let radius = match f.kind {
                Kind::Tree => {
                    let r = ((0.3 * f.height as f64).max(1.2) * ppm).max(1.0);
                    oval(pixmap, x, y, r, c.body);
                    if r >= 3.0 {
                        oval(pixmap, x - r * 0.3, y - r * 0.3, r * 0.45, c.top);
                    }
                    continue;
                }
                Kind::PowerTower => 3.0,
                Kind::WaterTower | Kind::Tower => 2.8,
                Kind::Chimney => 1.6,
                Kind::Bench | Kind::PicnicTable | Kind::Shelter | Kind::SubwayEntrance => 1.0,
                Kind::Shrub => 0.8,
                _ => 0.3,
            };
            oval(pixmap, x, y, (radius * ppm).max(1.0), c.top);
        }
    }
}
