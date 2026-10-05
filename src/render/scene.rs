//! The 3D scene: everything that stands above the ground, painted back to
//! front (painter's algorithm) so that nearer and taller things hide what is
//! behind them.
//!
//! Items are solids (buildings, parts, tanks, canopies), point objects (trees,
//! lamps, hydrants...), and chunks of raised lines (bridge decks, walls,
//! wires). Each is keyed by the screen depth of its lowest ground point; the
//! key is a pure function of the geometry and camera, so neighbouring tiles
//! paint shared items in the same order and stay seamless.

use super::camera::Frame;
use super::{Renderer, Scratch};
use crate::geo::Point;
use crate::map::Map;
use crate::style::LineStyle;
use tiny_skia::{Color, Pixmap};

/// Screen length of raised-line chunks. Shorter chunks sort more precisely
/// against their surroundings; longer ones show fewer joins.
const CHUNK_PX: f64 = 96.0;

/// How a raised line stands above the ground.
#[derive(Clone, Copy)]
pub(super) enum Form {
    /// A deck on pillars (bridges, viaducts, jet bridges, pipelines).
    Deck {
        /// Distance between pillars in meters.
        pillar_spacing: f64,
    },
    /// A vertical face (walls, hedges, fences, dams).
    Face(Color),
    /// A wire or beam strung between poles at its vertices.
    Wire(Color),
}

pub(super) struct RaisedLine {
    pub(super) style: LineStyle,
    pub(super) form: Form,
    /// Height of the line above the ground, in screen pixels.
    pub(super) raise: f64,
    /// Height of its bottom (`min_height`, e.g. a parapet on a roof), in
    /// screen pixels.
    pub(super) base: f64,
}

pub(super) enum Item {
    /// A solid or point object, by feature index.
    Feature(u32),
    /// `points[start..start + len]` of raised line `line`, starting
    /// `distance` pixels along it.
    Chunk {
        line: u32,
        start: u32,
        len: u32,
        distance: f64,
    },
    /// A tree of a tree row: position on the ground and height in meters.
    Tree { at: Point, height: f32 },
}

#[derive(Default)]
pub(super) struct Scene {
    items: Vec<(f64, u64, Item)>,
    pub(super) points: Vec<Point>,
    pub(super) lines: Vec<RaisedLine>,
    seq: u64,
}

impl Scene {
    fn push(&mut self, depth: f64, tie: u64, item: Item) {
        self.items.push((depth, tie, item));
    }

    /// Adds solids and point objects.
    pub(super) fn add_features(&mut self, fr: &Frame, map: &Map, run: &[u32]) {
        for &id in run {
            let f = &map.features[id as usize];
            let depth = if f.kind.is_point() {
                let p = map.ring(f.ring_start)[0];
                fr.apply(p[0] as f64, p[1] as f64)[1]
            } else {
                fr.depth(f)
            };
            self.push(depth, (id as u64) << 24, Item::Feature(id));
        }
    }

    /// Registers a raised line style and splits its geometry into chunks.
    /// `key` identifies the line for stable tie-breaking.
    pub(super) fn add_line(
        &mut self,
        line: RaisedLine,
        pieces: &[(Vec<Point>, f64)],
        key: u32,
        per_segment: bool,
    ) {
        let index = self.lines.len() as u32;
        self.lines.push(line);
        for (pts, distance) in pieces {
            let mut start = 0;
            let mut dist = *distance;
            let mut run_len = 0.0;
            for i in 1..pts.len() {
                let (a, b) = (pts[i - 1], pts[i]);
                run_len += ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
                if per_segment || run_len >= CHUNK_PX || i == pts.len() - 1 {
                    let chunk = &pts[start..=i];
                    let depth = chunk.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max);
                    let first = self.points.len() as u32;
                    self.points.extend_from_slice(chunk);
                    self.seq += 1;
                    self.push(
                        depth,
                        ((key as u64) << 24) | (self.seq & 0xff_ffff),
                        Item::Chunk {
                            line: index,
                            start: first,
                            len: chunk.len() as u32,
                            distance: dist,
                        },
                    );
                    dist += run_len;
                    run_len = 0.0;
                    start = i;
                }
            }
        }
    }

    pub(super) fn add_tree(&mut self, at: Point, height: f32, key: u32) {
        self.seq += 1;
        self.push(
            at[1],
            ((key as u64) << 24) | (self.seq & 0xff_ffff),
            Item::Tree { at, height },
        );
    }
}

impl Renderer<'_> {
    pub(super) fn draw_scene(
        &self,
        pixmap: &mut Pixmap,
        fr: &Frame,
        mut scene: Scene,
        s: &mut Scratch,
    ) {
        let mut items = std::mem::take(&mut scene.items);
        items.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        for (_, _, item) in items {
            match item {
                Item::Feature(id) => {
                    let f = &self.map.features[id as usize];
                    if f.kind.is_point() {
                        self.draw_prop(pixmap, fr, f);
                    } else {
                        self.draw_solid(pixmap, fr, f, s);
                    }
                }
                Item::Chunk {
                    line,
                    start,
                    len,
                    distance,
                } => {
                    let pts = &scene.points[start as usize..(start + len) as usize];
                    self.draw_raised(pixmap, fr, &scene.lines[line as usize], pts, distance);
                }
                Item::Tree { at, height } => self.draw_tree(pixmap, fr, at, 0.0, height, false),
            }
        }
    }
}
