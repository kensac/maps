//! A packed Hilbert R-tree (as in Flatbush): items sorted along a Hilbert
//! curve and grouped [`NODE_SIZE`] to a node, level by level up to one root.
//! The whole tree is two flat arrays, so it is stored in a snapshot and
//! queried straight from the mapped file.
//!
//! Nodes `0..len` are the items in curve order; each level follows the one
//! below it, and the root is last. `ids[i]` is an item's id at the leaves
//! and the first child of node `i` above them.

use crate::store::Store;
use rayon::prelude::*;

pub const NODE_SIZE: usize = 16;

pub struct Packed {
    /// Number of items.
    len: usize,
    /// End (exclusive) of each level's nodes, from the leaves up.
    levels: Vec<u32>,
    pub boxes: Store<[f32; 4]>,
    pub ids: Store<u32>,
}

/// Node counts per level for `len` items, cumulative from the leaves up.
pub fn levels(len: usize) -> Vec<u32> {
    let mut levels = vec![len as u32];
    let (mut n, mut total) = (len, len);
    while n > 1 {
        n = n.div_ceil(NODE_SIZE);
        total += n;
        levels.push(total as u32);
    }
    levels
}

fn intersects(b: &[f32; 4], q: &[f32; 4]) -> bool {
    b[0] <= q[2] && b[2] >= q[0] && b[1] <= q[3] && b[3] >= q[1]
}

impl Packed {
    /// Builds a tree over `items`, `(box, id)` pairs.
    pub fn build(items: Vec<([f32; 4], u32)>) -> Packed {
        let len = items.len();
        let levels = levels(len);
        if len == 0 {
            return Packed {
                len,
                levels,
                boxes: Store::Owned(Vec::new()),
                ids: Store::Owned(Vec::new()),
            };
        }
        let mut ext = [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        for (b, _) in &items {
            ext = union(&ext, b);
        }
        let (w, h) = (ext[2] - ext[0], ext[3] - ext[1]);
        let scale = |v: f32, lo: f32, span: f32| {
            if span > 0.0 {
                ((v - lo) / span * 65535.0) as u32
            } else {
                0
            }
        };
        let mut keyed: Vec<(u32, u32, [f32; 4])> = items
            .into_par_iter()
            .map(|(b, id)| {
                let x = scale((b[0] + b[2]) * 0.5, ext[0], w);
                let y = scale((b[1] + b[3]) * 0.5, ext[1], h);
                (hilbert(x, y), id, b)
            })
            .collect();
        // Ids are unique, so the order is fully determined.
        keyed.par_sort_unstable_by_key(|&(h, id, _)| (h, id));

        let total = *levels.last().unwrap() as usize;
        let mut boxes = Vec::with_capacity(total);
        let mut ids = Vec::with_capacity(total);
        for (_, id, b) in keyed {
            boxes.push(b);
            ids.push(id);
        }
        let mut start = 0;
        for &end in &levels[..levels.len() - 1] {
            let end = end as usize;
            for first in (start..end).step_by(NODE_SIZE) {
                let last = (first + NODE_SIZE).min(end);
                let b = boxes[first..last].iter().fold(
                    [
                        f32::INFINITY,
                        f32::INFINITY,
                        f32::NEG_INFINITY,
                        f32::NEG_INFINITY,
                    ],
                    |a, b| union(&a, b),
                );
                boxes.push(b);
                ids.push(first as u32);
            }
            start = end;
        }
        debug_assert_eq!(boxes.len(), total);
        Packed {
            len,
            levels,
            boxes: Store::Owned(boxes),
            ids: Store::Owned(ids),
        }
    }

    /// A tree from stored arrays, checked enough that queries cannot index
    /// out of bounds.
    pub fn from_parts(
        len: usize,
        boxes: Store<[f32; 4]>,
        ids: Store<u32>,
    ) -> anyhow::Result<Packed> {
        let levels = levels(len);
        let total = *levels.last().unwrap() as usize;
        anyhow::ensure!(
            boxes.len() == total && ids.len() == total,
            "index has {} nodes, expected {total}",
            boxes.len()
        );
        Ok(Packed {
            len,
            levels,
            boxes,
            ids,
        })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Calls `found` with the id of every item whose box intersects `q`
    /// (bounds inclusive), in no particular order.
    pub fn search(&self, q: &[f32; 4], stack: &mut Vec<usize>, mut found: impl FnMut(u32)) {
        if self.len == 0 {
            return;
        }
        stack.clear();
        let mut node = self.boxes.len() - 1;
        loop {
            let level_end = self.levels[self.levels.partition_point(|&e| e as usize <= node)];
            let end = (node + NODE_SIZE).min(level_end as usize);
            for pos in node..end {
                if !intersects(&self.boxes[pos], q) {
                    continue;
                }
                let id = self.ids[pos];
                if pos < self.len {
                    found(id);
                } else {
                    stack.push(id as usize);
                }
            }
            match stack.pop() {
                Some(n) => node = n,
                None => break,
            }
        }
    }
}

fn union(a: &[f32; 4], b: &[f32; 4]) -> [f32; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

/// Position of `(x, y)` (16 bits each) along a Hilbert curve; from
/// <https://github.com/rawrunprotected/hilbert_curves>, as Flatbush uses.
fn hilbert(x: u32, y: u32) -> u32 {
    let mut a = x ^ y;
    let mut b = 0xffff ^ a;
    let mut c = 0xffff ^ (x | y);
    let mut d = x & (y ^ 0xffff);

    let mut aa = a | (b >> 1);
    let mut bb = (a >> 1) ^ a;
    let mut cc = ((c >> 1) ^ (b & (d >> 1))) ^ c;
    let mut dd = ((a & (c >> 1)) ^ (d >> 1)) ^ d;

    a = aa;
    b = bb;
    c = cc;
    d = dd;
    aa = (a & (a >> 2)) ^ (b & (b >> 2));
    bb = (a & (b >> 2)) ^ (b & ((a ^ b) >> 2));
    cc ^= (a & (c >> 2)) ^ (b & (d >> 2));
    dd ^= (b & (c >> 2)) ^ ((a ^ b) & (d >> 2));

    a = aa;
    b = bb;
    c = cc;
    d = dd;
    aa = (a & (a >> 4)) ^ (b & (b >> 4));
    bb = (a & (b >> 4)) ^ (b & ((a ^ b) >> 4));
    cc ^= (a & (c >> 4)) ^ (b & (d >> 4));
    dd ^= (b & (c >> 4)) ^ ((a ^ b) & (d >> 4));

    a = aa;
    b = bb;
    c = cc;
    d = dd;
    cc ^= (a & (c >> 8)) ^ (b & (d >> 8));
    dd ^= (b & (c >> 8)) ^ ((a ^ b) & (d >> 8));

    a = cc ^ (cc >> 1);
    b = dd ^ (dd >> 1);

    let mut i0 = x ^ y;
    let mut i1 = b | (0xffff ^ (i0 | a));

    i0 = (i0 | (i0 << 8)) & 0x00ff00ff;
    i0 = (i0 | (i0 << 4)) & 0x0f0f0f0f;
    i0 = (i0 | (i0 << 2)) & 0x33333333;
    i0 = (i0 | (i0 << 1)) & 0x55555555;

    i1 = (i1 | (i1 << 8)) & 0x00ff00ff;
    i1 = (i1 | (i1 << 4)) & 0x0f0f0f0f;
    i1 = (i1 | (i1 << 2)) & 0x33333333;
    i1 = (i1 | (i1 << 1)) & 0x55555555;

    (i1 << 1) | i0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small deterministic generator, so the tests need no crate.
    pub(crate) struct Rng(pub u64);
    impl Rng {
        pub fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }
    }

    fn random_box(r: &mut Rng, size: f32) -> [f32; 4] {
        let (x, y) = (r.next(), r.next());
        let (w, h) = (r.next() * size, r.next() * size);
        [x, y, x + w, y + h]
    }

    #[test]
    fn search_matches_brute_force() {
        let mut r = Rng(0x9e3779b97f4a7c15);
        for n in [0, 1, 15, 16, 17, 255, 256, 257, 5000] {
            let items: Vec<_> = (0..n)
                .map(|i| {
                    // Some points (zero-size boxes), as point features are.
                    let size = if i % 3 == 0 { 0.0 } else { 0.05 };
                    (random_box(&mut r, size), i as u32)
                })
                .collect();
            let tree = Packed::build(items.clone());
            let mut stack = Vec::new();
            for _ in 0..200 {
                let q = random_box(&mut r, 0.2);
                let mut got = Vec::new();
                tree.search(&q, &mut stack, |id| got.push(id));
                got.sort_unstable();
                let want: Vec<u32> = items
                    .iter()
                    .filter(|(b, _)| intersects(b, &q))
                    .map(|&(_, id)| id)
                    .collect();
                assert_eq!(got, want, "n={n}");
            }
        }
    }

    #[test]
    fn level_counts() {
        assert_eq!(levels(0), vec![0]);
        assert_eq!(levels(1), vec![1]);
        assert_eq!(levels(16), vec![16, 17]);
        assert_eq!(levels(17), vec![17, 19, 20]);
    }

    #[test]
    fn hilbert_visits_each_cell_once() {
        let mut seen = std::collections::HashSet::new();
        for x in 0..64 {
            for y in 0..64 {
                assert!(seen.insert(hilbert(x, y)));
            }
        }
    }
}
