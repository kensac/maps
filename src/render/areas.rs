//! Filled areas, shoreline banks and the mask outside the data bounds.

use super::camera::Frame;
use super::paint::{paint, push_ring, stroke};
use super::{Renderer, Scratch};
use crate::classify::Kind;
use crate::geo::{clip_ring, point_in_ring, Point};
use tiny_skia::{BlendMode, Color, FillRule, PathBuilder, Pixmap, Transform};

/// How far water lies below the land, in meters.
const WATER_DEPTH: f64 = 3.0;

/// Vertical faces an area shows in the tilted view.
struct Banks {
    /// Height of the face in meters.
    depth: f64,
    /// Which edges get a face: those whose outward side faces the viewer
    /// (raised land and slabs), or away from it (the far shore of water).
    toward_viewer: bool,
    /// Whether the area's material lies on the left of its rings. Map
    /// polygons are oriented that way; coastline land runs the other way
    /// (OSM puts land on the left as seen on the map, where y points down).
    material_left: bool,
}

fn banks(kind: Kind) -> Option<Banks> {
    let (depth, toward_viewer, material_left) = match kind {
        Kind::Land => (WATER_DEPTH, true, false),
        Kind::Water => (WATER_DEPTH, false, true),
        Kind::Pier | Kind::BridgeArea => (WATER_DEPTH + 1.0, true, true),
        Kind::Platform => (1.0, true, true),
        _ => return None,
    };
    Some(Banks {
        depth,
        toward_viewer,
        material_left,
    })
}

/// Adds a face hanging `depth` pixels below each qualifying edge of `ring`.
fn push_banks(pb: &mut PathBuilder, ring: &[Point], b: &Banks, depth: f64) {
    let n = ring.len();
    if n < 3 {
        return;
    }
    for e in 0..n {
        let (a, c) = (ring[e], ring[(e + 1) % n]);
        let dx = c[0] - a[0];
        // Outward normal is (dy, -dx) with material on the left, else the
        // reverse; it faces down the screen (toward the viewer) when its
        // y component is positive.
        let faces_viewer = if b.material_left { dx < 0.0 } else { dx > 0.0 };
        if faces_viewer == b.toward_viewer && dx != 0.0 {
            let quad = [a, c, [c[0], c[1] + depth], [a[0], a[1] + depth]];
            push_ring(pb, &quad);
        }
    }
}

impl Renderer<'_> {
    pub(super) fn draw_areas(&self, pixmap: &mut Pixmap, fr: &Frame, run: &[u32], s: &mut Scratch) {
        for batch in self.batches(run) {
            let f0 = &self.map.features[batch[0] as usize];
            let Some(style) = self.theme.area(f0.kind, &fr.ctx) else {
                continue;
            };
            let bank = banks(f0.kind).filter(|b| b.depth * fr.lift_per_m >= 0.75);
            let depth = bank.as_ref().map_or(0.0, |b| b.depth * fr.lift_per_m);
            // Faces hang below edges; clip far enough out that faces from
            // artificial clip edges never reach into the view.
            let margin = style.outline.as_ref().map_or(0.0, |o| o.width as f64) + 2.0 + depth;
            let clip = fr.rect(margin);
            let mut pb = PathBuilder::new();
            let mut faces = PathBuilder::new();
            for &id in batch {
                let f = &self.map.features[id as usize];
                let inside = clip.contains([fr.bbox(f).min_x, fr.bbox(f).min_y])
                    && clip.contains([fr.bbox(f).max_x, fr.bbox(f).max_y]);
                for ring in self.map.rings(f) {
                    fr.project(ring, &mut s.pts);
                    if !inside {
                        clip_ring(&mut s.pts, &clip, &mut s.tmp);
                    }
                    push_ring(&mut pb, &s.pts);
                    if let Some(b) = &bank {
                        push_banks(&mut faces, &s.pts, b, depth);
                    }
                }
            }
            let Some(path) = pb.finish() else { continue };
            // Raised land and slabs: faces first, so the top covers them.
            let toward = bank.as_ref().is_some_and(|b| b.toward_viewer);
            let faces = faces.finish();
            let fill_faces = |pixmap: &mut Pixmap, faces: &Option<tiny_skia::Path>| {
                if let Some(f) = faces {
                    let p = paint(self.theme.bank());
                    pixmap.fill_path(f, &p, FillRule::Winding, Transform::identity(), None);
                }
            };
            if toward {
                fill_faces(pixmap, &faces);
            }
            pixmap.fill_path(
                &path,
                &paint(style.fill),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
            if let Some(outline) = &style.outline {
                pixmap.stroke_path(
                    &path,
                    &paint(outline.color),
                    &stroke(outline, 0.0),
                    Transform::identity(),
                    None,
                );
            }
            if !toward {
                fill_faces(pixmap, &faces);
            }
        }
    }

    /// Paints everything outside the data bounds with the theme's "outside"
    /// color, or clears it to transparent.
    pub(super) fn mask_outside(&self, pixmap: &mut Pixmap, fr: &Frame) {
        let b = &self.map.bounds;
        let (ox, oy) = (self.map.origin[0], self.map.origin[1]);
        let (x0, y0, x1, y1) = (b.min_x - ox, b.min_y - oy, b.max_x - ox, b.max_y - oy);
        let poly = [
            fr.apply(x0, y0),
            fr.apply(x1, y0),
            fr.apply(x1, y1),
            fr.apply(x0, y1),
        ];
        let (w, h) = (fr.width, fr.height);
        let view = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]];
        if view.iter().all(|&p| point_in_ring(p, &poly)) {
            return;
        }
        let mut p = match self.theme.outside_color() {
            Some(c) => paint(c),
            None => paint(Color::TRANSPARENT),
        };
        p.blend_mode = BlendMode::Source;
        let mut pb = PathBuilder::new();
        push_ring(&mut pb, &view);
        push_ring(&mut pb, &poly);
        if let Some(path) = pb.finish() {
            pixmap.fill_path(&path, &p, FillRule::EvenOdd, Transform::identity(), None);
        }
    }
}
