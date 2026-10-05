//! Draws a viewport of a [`Map`] into a pixmap.
//!
//! Per viewport: query the zoom-bucketed index, walk the (pre-sorted) features
//! in runs that share a draw group and layer, batch consecutive features with
//! identical style into one path, and rasterize with anti-aliasing. Geometry is
//! projected, decimated to sub-pixel tolerance and clipped to the viewport
//! (plus a margin for stroke width) before it reaches the rasterizer, so huge
//! polygons cost little in tiles that only see a corner of them.
//!
//! In a tilted view, everything on the ground is painted first; everything
//! that stands above it (buildings, solids, objects, decks, walls, wires) goes
//! into a [`scene::Scene`] painted back to front, so nearer and taller things
//! hide what is behind them.
//!
//! Modules: [`camera`] (view transform), `areas`, `lines`, `solids`, `props`
//! and `scene`.

mod areas;
pub mod camera;
mod lines;
mod mesh;
mod models;
mod paint;
mod scene;
mod solids;

pub use camera::Viewport;

use crate::classify::{Group, Kind};
use crate::geo::{meters_per_unit, Point};
use crate::map::Map;
use crate::style::{Ctx, Theme};
use camera::Frame;
use scene::Scene;
use tiny_skia::Pixmap;

/// Buildings cast shadows from this zoom on in flat views, fading in over
/// one level.
const SHADOW_ZOOM: f32 = 14.5;
/// Pitch of the default oblique view used by static tiles: heights rise at
/// sin(pitch) = 0.6 of their true scale while the ground stays undistorted,
/// so the tiles still line up with a standard Web Mercator map.
pub const OBLIQUE_PITCH: f64 = 36.869_897_645_844_02;
/// Highest supported pitch.
pub const MAX_PITCH: f64 = 60.0;
/// Below this pitch the view is treated as straight down (flat).
const MIN_3D_PITCH: f64 = 0.5;
/// Deck height per OSM `layer` for bridges and viaducts. OSM almost never
/// records deck elevations, so this is an estimate from the layer.
const METERS_PER_LAYER: f64 = 5.5;

/// How buildings are drawn at high zoom.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, clap::ValueEnum)]
pub enum Buildings {
    /// Flat footprints with cast shadows.
    Flat,
    /// 3D: every solid and object rises to its true height.
    #[default]
    Extruded,
}

/// Reusable per-thread buffers.
#[derive(Default)]
struct Scratch {
    ids: Vec<u32>,
    pts: Vec<Point>,
    tmp: Vec<Point>,
}

pub struct Renderer<'a> {
    map: &'a Map,
    theme: &'a Theme,
    buildings: Buildings,
}

impl<'a> Renderer<'a> {
    pub fn new(map: &'a Map, theme: &'a Theme, buildings: Buildings) -> Self {
        Renderer {
            map,
            theme,
            buildings,
        }
    }

    pub fn render(&self, vp: &Viewport) -> Pixmap {
        let mut pixmap = Pixmap::new(vp.width, vp.height).expect("viewport has a non-zero size");
        self.render_into(vp, &mut pixmap);
        pixmap
    }

    /// Whether the viewport could show any map data (including buildings
    /// rising into it from below).
    pub fn intersects(&self, vp: &Viewport) -> bool {
        let below = match self.buildings {
            Buildings::Extruded => {
                let k = vp.world_px();
                let c = vp.inverse([vp.x + vp.width as f64 / 2.0, vp.y + vp.height as f64 / 2.0]);
                let ppm = k / meters_per_unit((c[1] / k).clamp(0.0, 1.0));
                self.map.max_height as f64 * ppm * vp.lift()
            }
            Buildings::Flat => 0.0,
        };
        vp.world_bounds(0.0, below).intersects(&self.map.bounds)
    }

    pub fn render_into(&self, vp: &Viewport, pixmap: &mut Pixmap) {
        let map = self.map;
        let k = vp.world_px();
        let (cos, sin) = vp.rotation();
        let gs = vp.ground_scale();
        let center = vp.inverse([vp.x + vp.width as f64 / 2.0, vp.y + vp.height as f64 / 2.0]);
        let origin = vp.forward([map.origin[0] * k, map.origin[1] * k]);
        let ppm = k / meters_per_unit((center[1] / k).clamp(0.0, 1.0));
        let frame = Frame {
            k,
            bearing_sc: (sin, cos),
            pitch_sc: (vp.pitch.to_radians().sin(), vp.pitch.to_radians().cos()),
            a: k * cos,
            b: k * sin,
            c: -k * sin * gs,
            d: k * cos * gs,
            bx: origin[0] - vp.x,
            by: origin[1] - vp.y,
            lift_per_m: ppm * vp.lift(),
            width: vp.width as f64,
            height: vp.height as f64,
            ctx: Ctx {
                zoom: vp.zoom as f32,
                scale: vp.scale,
                ppm: ppm as f32,
            },
            vis_zoom: (vp.zoom + (vp.scale as f64).log2()) as f32,
        };

        pixmap.fill(match map.background {
            crate::assemble::Background::Land => self.theme.land_color(),
            crate::assemble::Background::Water => self.theme.water_color(),
        });

        let mut s = Scratch::default();
        let three_d = self.buildings == Buildings::Extruded && vp.pitch >= MIN_3D_PITCH;
        // Query generously: wide strokes and shadows reach beyond feature
        // bounds, and in 3D anything south of the view may rise into it.
        let margin = 160.0 * vp.scale as f64;
        let tallest = map.max_height.max(5.0 * METERS_PER_LAYER as f32) as f64;
        let below = if three_d {
            margin + tallest * frame.lift_per_m
        } else {
            margin
        };
        let world = vp.world_bounds(margin, below);
        let area = [
            (world.min_x - map.origin[0]) as f32,
            (world.min_y - map.origin[1]) as f32,
            (world.max_x - map.origin[0]) as f32,
            (world.max_y - map.origin[1]) as f32,
        ];
        map.query(area, frame.vis_zoom, &mut s.ids);
        let mut ids = std::mem::take(&mut s.ids);
        if below > margin {
            // The query reached far below the view; keep only what is on
            // screen, or stands tall enough to rise into it.
            let view = frame.rect(margin);
            ids.retain(|&id| {
                let f = &map.features[id as usize];
                let b = frame.bbox(f);
                if b.intersects(&view) {
                    return true;
                }
                let top = match f.group() {
                    Group::Objects => f.height as f64,
                    Group::TransportBridges => f.layer.max(1) as f64 * METERS_PER_LAYER,
                    Group::Overlays => f.height as f64,
                    _ => return false,
                };
                b.min_y - top * frame.lift_per_m <= view.max_y
                    && b.max_x >= view.min_x
                    && b.min_x <= view.max_x
            });
        }

        let mut scene = Scene::default();
        let mut i = 0;
        while i < ids.len() {
            let first = &map.features[ids[i] as usize];
            let group = first.group();
            let layer = first.layer;
            let transport = group.is_transport();
            let mut j = i + 1;
            while j < ids.len() {
                let f = &map.features[ids[j] as usize];
                if f.group() != group || (transport && f.layer != layer) {
                    break;
                }
                j += 1;
            }
            let run = &ids[i..j];
            match group {
                Group::Land | Group::Areas | Group::AreaOverlays => {
                    self.draw_areas(pixmap, &frame, run, &mut s)
                }
                // Underground: hidden from a tilted view.
                Group::TransportTunnels if three_d => {}
                Group::Objects if three_d => scene.add_features(&frame, map, run),
                Group::Objects => self.draw_objects_flat(pixmap, &frame, run, &mut s),
                _ => {
                    let scene = three_d.then_some(&mut scene);
                    self.draw_lines(pixmap, &frame, run, &mut s, scene)
                }
            }
            i = j;
        }
        if three_d {
            self.draw_scene(pixmap, &frame, scene, &mut s);
        }

        self.mask_outside(pixmap, &frame);
    }

    /// Splits a run into batches of consecutive features drawn identically.
    fn batches<'r>(&'r self, run: &'r [u32]) -> impl Iterator<Item = &'r [u32]> + 'r {
        let features = &self.map.features;
        run.chunk_by(move |&a, &b| {
            let (a, b) = (&features[a as usize], &features[b as usize]);
            a.kind == b.kind
                && a.flags == b.flags
                && (a.kind != Kind::Boundary || a.height == b.height)
        })
    }
}
