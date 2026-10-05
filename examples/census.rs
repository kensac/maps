//! Tag census: counts every `key=value` in an extract, per element type, to
//! audit what physical features a region maps (and what the renderer might
//! still be missing).
//!
//! ```sh
//! cargo run --release --example census -- nyc.osm.pbf > census.txt
//! ```
//!
//! Prints one line per key with at least 200 uses: element type (N/W/R),
//! key, total count, and its most common values.

use osmpbf::{Element, ElementReader};
use std::cmp::Reverse;
use std::collections::HashMap;

/// `(element type, key, value) → count`.
type Counts = HashMap<(u8, String, String), u64>;
/// `(element type, key) → (total, [(value, count)])`.
type ByKey = HashMap<(u8, String), (u64, Vec<(String, u64)>)>;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: census <file.osm.pbf>");
    let reader = ElementReader::from_path(&path).expect("opening input");
    let counts: Counts = reader
        .par_map_reduce(
            |element| {
                let mut c = Counts::new();
                let (t, tags): (u8, Vec<(&str, &str)>) = match &element {
                    Element::Node(n) => (0, n.tags().collect()),
                    Element::DenseNode(n) => (0, n.tags().collect()),
                    Element::Way(w) => (1, w.tags().collect()),
                    Element::Relation(r) => (2, r.tags().collect()),
                };
                for (k, v) in tags {
                    let v = if v.len() > 40 { "…" } else { v };
                    *c.entry((t, k.to_string(), v.to_string())).or_default() += 1;
                }
                c
            },
            Counts::new,
            |mut a, b| {
                for (k, v) in b {
                    *a.entry(k).or_default() += v;
                }
                a
            },
        )
        .expect("reading input");

    let mut keys = ByKey::new();
    for ((t, k, v), n) in counts {
        let e = keys.entry((t, k)).or_default();
        e.0 += n;
        e.1.push((v, n));
    }
    let mut keys: Vec<_> = keys.into_iter().collect();
    keys.sort_by_key(|(_, (n, _))| Reverse(*n));
    for ((t, k), (n, mut values)) in keys {
        if n < 200 {
            continue;
        }
        values.sort_by_key(|(_, c)| Reverse(*c));
        let top: Vec<String> = values
            .iter()
            .take(14)
            .map(|(v, c)| format!("{v}:{c}"))
            .collect();
        println!(
            "{} {k} [{n}] {}",
            ["N", "W", "R"][t as usize],
            top.join(" ")
        );
    }
}
