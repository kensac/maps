//! Smooth elevation profiles for roads and rails.
//!
//! Bridge decks keep their layer height; height then spreads through the
//! connected network, dropping at most 5% (roads) or 2.5% (rail) per meter,
//! so approaches become ramps instead of steps.

use crate::classify::{flags, Kind};
use crate::geo::meters_per_unit;
use crate::ingest::RawData;
use rustc_hash::FxHashMap;
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// Deck height per OSM `layer`, in meters (OSM records no deck elevations).
pub const METERS_PER_LAYER: f32 = 5.5;

/// Steepest climb, as rise per meter run.
fn grade(kind: Kind) -> f64 {
    match kind {
        Kind::Rail | Kind::Subway | Kind::LightRail | Kind::Tram => 0.025,
        _ => 0.05,
    }
}

/// Deck height a way is mapped at, in meters.
pub fn deck_height(kind: Kind, layer: i8, f: u8) -> f32 {
    let raised = layer > 0 || f & flags::BRIDGE != 0;
    if kind.is_transport() && raised && f & flags::TUNNEL == 0 {
        layer.max(1) as f32 * METERS_PER_LAYER
    } else {
        0.0
    }
}

#[derive(PartialEq)]
struct Item(f64, i64);

impl Eq for Item {}

impl PartialOrd for Item {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Item {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0).then(self.1.cmp(&other.1))
    }
}

/// Per-way, per-ref elevations in meters, for ways that leave the ground
/// anywhere (indexed like `raw.ways`).
pub fn profiles(raw: &RawData) -> FxHashMap<usize, Vec<f32>> {
    let on_network = |i: usize| {
        let w = &raw.ways[i];
        w.kind.is_transport() && w.flags & flags::TUNNEL == 0
    };
    // Node → (way, ref index) for every node of a transport way.
    let mut at_node: FxHashMap<i64, Vec<(u32, u32)>> = FxHashMap::default();
    let mut height: FxHashMap<i64, f64> = FxHashMap::default();
    let mut heap = BinaryHeap::new();
    let mut any_deck = false;
    for (i, w) in raw.ways.iter().enumerate() {
        if !on_network(i) {
            continue;
        }
        let deck = deck_height(w.kind, w.layer, w.flags) as f64;
        any_deck |= deck > 0.0;
        if deck > 0.0 {
            for &id in &w.refs {
                let h = height.entry(id).or_insert(0.0);
                if deck > *h {
                    *h = deck;
                    heap.push(Item(deck, id));
                }
            }
        }
    }
    if !any_deck {
        return FxHashMap::default();
    }
    for (i, w) in raw.ways.iter().enumerate() {
        if on_network(i) {
            for (j, &id) in w.refs.iter().enumerate() {
                at_node.entry(id).or_default().push((i as u32, j as u32));
            }
        }
    }

    // Spread heights outward, losing `grade` per meter.
    while let Some(Item(h, id)) = heap.pop() {
        if height.get(&id).is_some_and(|&cur| cur > h) {
            continue;
        }
        let Some(p) = raw.node(id) else { continue };
        let mpu = meters_per_unit(p[1]);
        for &(w, j) in at_node.get(&id).map(Vec::as_slice).unwrap_or(&[]) {
            let way = &raw.ways[w as usize];
            let g = grade(way.kind);
            let neighbours = [j.checked_sub(1), Some(j + 1)];
            for k in neighbours.into_iter().flatten() {
                let Some(&nid) = way.refs.get(k as usize) else {
                    continue;
                };
                let Some(q) = raw.node(nid) else { continue };
                let run = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)).sqrt() * mpu;
                let nh = h - g * run;
                if nh > 0.05 && height.get(&nid).is_none_or(|&cur| nh > cur) {
                    height.insert(nid, nh);
                    heap.push(Item(nh, nid));
                }
            }
        }
    }

    let mut out = FxHashMap::default();
    for (i, w) in raw.ways.iter().enumerate() {
        if !on_network(i) {
            continue;
        }
        let e: Vec<f32> = w
            .refs
            .iter()
            .map(|id| height.get(id).copied().unwrap_or(0.0) as f32)
            .collect();
        if e.iter().any(|&h| h > 0.05) {
            out.insert(i, e);
        }
    }
    out
}
