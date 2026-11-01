use super::*;
use crate::classify::Detail;
use crate::index::NODE_SIZE;
use rstar::primitives::{GeomWithData, Rectangle};
use rstar::{RTree, AABB};

/// A small deterministic generator, so the tests need no crate.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
    fn below(&mut self, n: usize) -> usize {
        ((self.next() * n as f32) as usize).min(n - 1)
    }
}

type Entry = GeomWithData<Rectangle<[f32; 2]>, u32>;

/// The index as it was before v2: one rstar tree per zoom bucket, filtered
/// by `vis_zoom`. The reference the packed trees must agree with.
struct Reference(Vec<RTree<Entry>>);

impl Reference {
    fn new(map: &Map) -> Self {
        let mut buckets: Vec<Vec<Entry>> = (0..=MAX_BUCKET).map(|_| Vec::new()).collect();
        for (i, f) in map.features.iter().enumerate() {
            let b = (f.vis_zoom.max(0.0) as usize).min(MAX_BUCKET);
            let r = Rectangle::from_corners([f.bbox[0], f.bbox[1]], [f.bbox[2], f.bbox[3]]);
            buckets[b].push(GeomWithData::new(r, i as u32));
        }
        Reference(buckets.into_iter().map(RTree::bulk_load).collect())
    }

    fn query(&self, map: &Map, area: [f32; 4], zoom: f32) -> Vec<u32> {
        let env = AABB::from_corners([area[0], area[1]], [area[2], area[3]]);
        let last = (zoom.max(0.0) as usize).min(MAX_BUCKET);
        let mut out = Vec::new();
        for tree in &self.0[..=last] {
            out.extend(
                tree.locate_in_envelope_intersecting(env)
                    .map(|e| e.data)
                    .filter(|&i| map.features[i as usize].vis_zoom <= zoom),
            );
        }
        out.sort_unstable();
        out
    }
}

/// A map of random features over the unit square, every kind and roof
/// shape represented.
fn synthetic(n: usize, seed: u64) -> Map {
    let mut r = Rng(seed);
    let mut points = Vec::new();
    let mut ring_starts = Vec::new();
    let mut features = Vec::new();
    let mut details = Vec::new();
    for i in 0..n {
        let (x, y) = (r.next(), r.next());
        let size = if i % 4 == 0 { 0.0 } else { r.next() * 0.02 };
        ring_starts.push(points.len() as u32);
        points.extend([[x, y], [x + size, y], [x + size, y + size]]);
        let mut f: Feature = Zeroable::zeroed();
        f.set_kind(Kind::from_u8((i % Kind::COUNT) as u8).unwrap());
        f.ring_start = i as u32;
        f.ring_count = 1;
        f.layer = (i % 5) as i8 - 2;
        f.detail = u32::MAX;
        f.elev = u32::MAX;
        f.group = u32::MAX;
        f.heading = f32::NAN;
        // Never visible: left out of the index, filtered by the reference.
        f.vis_zoom = if i % 97 == 0 {
            f32::NAN
        } else {
            r.next() * 23.0 - 1.0
        };
        f.bbox = [x, y, x + size, y + size];
        if i % 7 == 0 {
            f.detail = details.len() as u32;
            details.push(DetailRec::new(&Detail {
                roof: RoofShape::from_u8((i % 6) as u8).unwrap(),
                roof_height: 2.5,
                across: i % 2 == 0,
                roof_colour: (i % 3 == 0).then_some(0x8a4b38),
                facade_colour: None,
                levels: 4.0,
            }));
        }
        features.push(f);
    }
    ring_starts.push(points.len() as u32);
    Map::from_parts(
        Rect::new(0.25, 0.25, 0.5, 0.5),
        [0.25, 0.25],
        Background::Water,
        points,
        ring_starts,
        features,
        321.0,
        details,
        vec![1.0, 2.0, 3.0],
        vec![[0.0, 0.0, 1.0, 1.0]],
    )
}

/// A directory for one test's files, removed when the test ends.
struct Temp(PathBuf);
impl Temp {
    fn new(test: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("maps-test-{}-{test}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn same_arrays(a: &Map, b: &Map) {
    fn bytes<T: Pod>(s: &[T]) -> &[u8] {
        bytemuck::cast_slice(s)
    }
    assert_eq!(bytes(&a.points), bytes(&b.points));
    assert_eq!(bytes(&a.ring_starts), bytes(&b.ring_starts));
    assert_eq!(bytes(&a.features), bytes(&b.features));
    assert_eq!(bytes(&a.details), bytes(&b.details));
    assert_eq!(bytes(&a.elevations), bytes(&b.elevations));
    assert_eq!(bytes(&a.groups), bytes(&b.groups));
    assert_eq!(a.stats(), b.stats());
    assert_eq!(
        (a.bounds, a.origin, a.background, a.max_height),
        (b.bounds, b.origin, b.background, b.max_height)
    );
}

/// The bounding box of every feature, in local coordinates.
fn extent(map: &Map) -> [f32; 4] {
    map.features.iter().fold(
        [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ],
        |e, f| {
            [
                e[0].min(f.bbox[0]),
                e[1].min(f.bbox[1]),
                e[2].max(f.bbox[2]),
                e[3].max(f.bbox[3]),
            ]
        },
    )
}

/// `query` agrees with the rstar reference over random boxes and zooms.
fn queries_match(map: &Map, reference: &Reference, mut r: Rng, area: [f32; 4], tries: usize) {
    let mut out = Vec::new();
    let (w, h) = (area[2] - area[0], area[3] - area[1]);
    for _ in 0..tries {
        let (x, y) = (area[0] + r.next() * w, area[1] + r.next() * h);
        let size = r.next().powi(3) * w.max(h) * 0.5;
        let q = [x, y, x + size, y + size * (0.5 + r.next())];
        // Whole, fractional and out-of-range zooms.
        let zoom = match r.below(4) {
            0 => r.below(22) as f32,
            1 => r.next() * 21.0,
            2 => -1.0 + r.next(),
            _ => 20.0 + r.next() * 5.0,
        };
        map.query(q, zoom, &mut out);
        assert_eq!(out, reference.query(map, q, zoom), "{q:?} at z{zoom}");
    }
}

#[test]
fn convert_matches_save_and_reads_back() {
    let tmp = Temp::new("convert_matches_save_and_reads_back");
    let map = synthetic(20_000, 0x2545f4914f6cdd1d);
    let (v1, saved, converted) = (tmp.path("a.snap1"), tmp.path("b.snap"), tmp.path("c.snap"));
    save_v1(&map, &v1).unwrap();
    save(&map, &saved).unwrap();
    convert(&v1, &converted).unwrap();
    // Building straight to v2 and converting from v1 write the same file.
    assert_eq!(
        std::fs::read(&saved).unwrap(),
        std::fs::read(&converted).unwrap()
    );
    assert!(convert(&converted, &tmp.path("d.snap")).is_err());

    let from_v1 = load(&v1).unwrap();
    let from_v2 = load(&converted).unwrap();
    assert!(!from_v1.features.is_mapped() && from_v2.features.is_mapped());
    same_arrays(&map, &from_v1);
    same_arrays(&map, &from_v2);
    for f in from_v2.features.iter().take(500) {
        assert_eq!(from_v2.detail(f), map.detail(f));
    }
    let reference = Reference::new(&from_v1);
    for m in [&map, &from_v1, &from_v2] {
        queries_match(m, &reference, Rng(7), [0.0, 0.0, 1.0, 1.0], 2000);
    }
}

#[test]
fn damaged_files_are_errors() {
    let tmp = Temp::new("damaged_files_are_errors");
    let map = synthetic(3000, 99);
    let path = tmp.path("damaged.snap");
    save(&map, &path).unwrap();
    let good = std::fs::read(&path).unwrap();
    let try_load = |bytes: &[u8], name: &str| {
        let p = tmp.path(name);
        std::fs::write(&p, bytes).unwrap();
        load(&p).map(|_| ())
    };
    assert!(try_load(&good, "ok.snap").is_ok());
    assert!(try_load(&good[..good.len() - 4], "short.snap").is_err());
    assert!(try_load(&good[..100], "header.snap").is_err());
    // A section moved off its alignment.
    let mut bad = good.clone();
    let at = std::mem::offset_of!(Header, sections) + FEATURES * 16;
    bad[at] = bad[at].wrapping_add(4);
    assert!(try_load(&bad, "misaligned.snap").is_err());
    // An unknown kind in a v1 file is refused when read or converted.
    let v1 = tmp.path("badkind.snap1");
    save_v1(&map, &v1).unwrap();
    let mut b = std::fs::read(&v1).unwrap();
    let features_at = 61 + 8 + map.points.len() * 8 + 8 + map.ring_starts.len() * 4 + 8;
    b[features_at] = 255;
    assert!(try_load(&b, "badkind2.snap1").is_err());

    // Damaged index and ring starts are refused at open, not at query time.
    let h: Header = bytemuck::pod_read_unaligned(&good[..std::mem::size_of::<Header>()]);
    let u32_at = |s: usize, i: usize| h.sections[s][0] as usize + 4 * i;
    let put = |bytes: &mut Vec<u8>, at: usize, v: u32| {
        bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
    };
    let lens: Vec<u64> = (0..=MAX_BUCKET)
        .map(|b| {
            let at = h.sections[INDEX_LENS][0] as usize + 8 * b;
            u64::from_le_bytes(good[at..at + 8].try_into().unwrap())
        })
        .collect();
    // The first internal node of the first bucket that has one.
    let mut node = 0;
    let mut internal = None;
    for &len in &lens {
        let nodes = *index::levels(len as usize).last().unwrap() as usize;
        if len as usize > NODE_SIZE {
            internal = Some((node, node + len as usize));
            break;
        }
        node += nodes;
    }
    let (leaf, internal) = internal.expect("a bucket with internal nodes");
    let n = map.features.len() as u32;
    for (what, at, v, error) in [
        (
            "leaf id past the features",
            u32_at(INDEX_IDS, leaf),
            n,
            "past the",
        ),
        (
            "internal id past the end",
            u32_at(INDEX_IDS, internal),
            u32::MAX,
            "points at",
        ),
        (
            "internal node pointing at itself",
            u32_at(INDEX_IDS, internal),
            (internal - node) as u32,
            "points at",
        ),
        (
            "ring start past the points",
            u32_at(RING_STARTS, 1),
            u32::MAX,
            "ring starts",
        ),
    ] {
        let mut bad = good.clone();
        put(&mut bad, at, v);
        match try_load(&bad, "bad.snap") {
            Ok(()) => panic!("{what} was accepted"),
            Err(e) => assert!(format!("{e:#}").contains(error), "{what}: {e:#}"),
        }
    }
}

/// v1 records, as v1 wrote them field by field, read as v2's `Feature`
/// and `DetailRec`: the conversion copies them byte for byte.
/// A v1 snapshot written by the last v1 build (bb62e44) from
/// `times-square.osm.pbf` (a block of Midtown; data © OpenStreetMap
/// contributors, ODbL). Reads with the counts that build's `maps info`
/// printed, converts to the same arrays, and this build's v1 writer
/// reproduces it byte for byte from the same extract.
#[test]
fn old_v1_snapshot_reads_and_converts() {
    let tmp = Temp::new("old_v1_snapshot_reads_and_converts");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let v1 = fixture.join("times-square.v1.snap");
    let old = load(&v1).unwrap();
    assert_eq!((old.features.len(), old.points.len()), (195, 1454));
    let names: Vec<(String, usize)> = old
        .stats()
        .into_iter()
        .map(|(k, n)| (format!("{k:?}"), n))
        .collect();
    let want = [
        ("Building", 89),
        ("Footway", 42),
        ("Kerb", 17),
        ("Subway", 11),
        ("TrafficSignal", 10),
        ("Secondary", 9),
        ("PedestrianStreet", 3),
        ("Minor", 3),
        ("Bench", 3),
        ("WasteBasket", 3),
        ("PedestrianArea", 2),
        ("Service", 2),
        ("BicycleParking", 1),
    ];
    let mut got = names.clone();
    let mut want: Vec<(String, usize)> = want.iter().map(|&(k, n)| (k.to_string(), n)).collect();
    got.sort();
    want.sort();
    assert_eq!(got, want);

    let v2 = tmp.path("times-square.snap");
    convert(&v1, &v2).unwrap();
    let new = load(&v2).unwrap();
    same_arrays(&old, &new);
    let reference = Reference::new(&old);
    queries_match(&new, &reference, Rng(3), extent(&old), 2000);

    let built = Map::build(crate::ingest::read(&fixture.join("times-square.osm.pbf")).unwrap());
    let rewritten = tmp.path("rewritten.snap1");
    save_v1(&built, &rewritten).unwrap();
    assert_eq!(
        std::fs::read(&rewritten).unwrap(),
        std::fs::read(&v1).unwrap()
    );
}

/// Feature records are not scanned at open, so out-of-range ring, detail,
/// elevation and group indices must draw nothing rather than panic.
#[test]
fn damaged_features_render_without_panicking() {
    use crate::render::{Buildings, Renderer, Viewport};
    let tmp = Temp::new("damaged_features_render_without_panicking");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let v2 = tmp.path("ts.snap");
    convert(&fixture.join("times-square.v1.snap"), &v2).unwrap();
    let mut bytes = std::fs::read(&v2).unwrap();
    let h: Header = bytemuck::pod_read_unaligned(&bytes[..std::mem::size_of::<Header>()]);
    let [at, n] = h.sections[FEATURES];
    for i in 0..n as usize {
        let f = at as usize + 56 * i;
        // ring_start, ring_count, detail, elev, group
        let v = [u32::MAX - 1, 5, 1 << 30, u32::MAX - 2, 1 << 30];
        for (k, v) in v.iter().enumerate() {
            bytes[f + 4 + 4 * k..f + 8 + 4 * k].copy_from_slice(&v.to_le_bytes());
        }
    }
    std::fs::write(&v2, &bytes).unwrap();
    let map = load(&v2).unwrap();
    let b = map.bounds;
    for z in [14u8, 16, 18] {
        let n = (1u64 << z) as f64;
        let x = (b.center()[0] * n) as u32;
        let y = (b.center()[1] * n) as u32;
        for theme in [
            crate::style::ThemeName::Light,
            crate::style::ThemeName::Dark,
        ] {
            let t = theme.theme();
            for buildings in [Buildings::Flat, Buildings::Extruded] {
                for pitch in [0.0, 45.0] {
                    let vp = Viewport::camera_tile(z, x as i64, y as i64, 1.0, 20.0, pitch);
                    Renderer::new(&map, t, buildings).render(&vp);
                }
            }
            let gz = z.min(crate::vtile::MAX_ZOOM);
            let shift = z - gz;
            crate::vtile::build(&map, t, gz, x >> shift, y >> shift);
        }
    }
}

#[test]
fn v1_records_are_v2_records() {
    let mut out = vec![Kind::Building as u8, 3, (-2i8) as u8, 1];
    for v in [7u32, 2, 9, u32::MAX, 5] {
        out.extend(v.to_le_bytes());
    }
    for v in [120.5f32, 4.0, f32::NAN, 13.5, 0.1, 0.2, 0.3, 0.4] {
        out.extend(v.to_le_bytes());
    }
    let f: Feature = bytemuck::pod_read_unaligned(&out);
    assert_eq!(
        (f.kind(), f.flags, f.layer, f.variant),
        (Kind::Building, 3, -2, 1)
    );
    assert_eq!(
        (f.ring_start, f.ring_count, f.detail, f.elev, f.group),
        (7, 2, 9, u32::MAX, 5)
    );
    assert!(f.heading.is_nan() && f.vis_zoom == 13.5 && f.bbox == [0.1, 0.2, 0.3, 0.4]);

    let d = Detail {
        roof: RoofShape::Hipped,
        roof_height: 3.5,
        across: true,
        roof_colour: Some(0x8a4b38),
        facade_colour: None,
        levels: 12.0,
    };
    let mut out = vec![d.roof as u8, 1, 1, 0];
    for v in [3.5f32, 12.0] {
        out.extend(v.to_le_bytes());
    }
    for v in [0x8a4b38u32, 0, 0] {
        out.extend(v.to_le_bytes());
    }
    assert_eq!(bytemuck::bytes_of(&DetailRec::new(&d)), &out[..]);
    assert_eq!(bytemuck::pod_read_unaligned::<DetailRec>(&out).get(), d);
}

#[test]
fn invalid_bytes_read_safely() {
    let mut f: Feature = Zeroable::zeroed();
    bytemuck::bytes_of_mut(&mut f)[0] = 250;
    assert_eq!(f.kind(), Kind::Land);
    let mut d = DetailRec::default();
    bytemuck::bytes_of_mut(&mut d)[..2].copy_from_slice(&[77, 9]);
    assert_eq!((d.get().roof, d.get().across), (RoofShape::Flat, true));
    for v in 0..Kind::COUNT as u8 {
        assert_eq!(Kind::from_u8(v).unwrap() as u8, v);
    }
    assert!(Kind::from_u8(Kind::COUNT as u8).is_none());
}

#[test]
fn trees_use_node_size() {
    let map = synthetic(NODE_SIZE * NODE_SIZE + 3, 5);
    let total: usize = map.index.iter().map(|t| t.len()).sum();
    let visible = map.features.iter().filter(|f| !f.vis_zoom.is_nan()).count();
    assert!(visible < map.features.len());
    assert_eq!(total, visible);
}

/// On a real extract: `MAPS_TEST_PBF=jfk.osm.pbf cargo test --release real_
/// -- --nocapture`. Queries on v2 match rstar on v1, and tiles from both are
/// byte-identical. Skipped without the variable.
#[test]
fn real_extract_v1_and_v2_agree() {
    let tmp = Temp::new("real_extract_v1_and_v2_agree");
    let Ok(pbf) = std::env::var("MAPS_TEST_PBF") else {
        eprintln!("MAPS_TEST_PBF not set; skipped");
        return;
    };
    use crate::render::{Buildings, Renderer, Viewport};
    use crate::style::ThemeName;

    let built = Map::build(crate::ingest::read(Path::new(&pbf)).unwrap());
    let (v1, v2) = (tmp.path("real.snap1"), tmp.path("real.snap"));
    save_v1(&built, &v1).unwrap();
    drop(built);
    convert(&v1, &v2).unwrap();
    let a = load(&v1).unwrap();
    let b = load(&v2).unwrap();
    same_arrays(&a, &b);

    let reference = Reference::new(&a);
    let extent = extent(&a);
    queries_match(&b, &reference, Rng(0xdeadbeef), extent, 20_000);
    eprintln!("20000 random queries agree");

    // Sample tiles across the extract, zoom 10 to 18.
    let mut r = Rng(42);
    let (mut tiles, mut drawn) = (0, 0);
    let mut ids = Vec::new();
    let mut pixmap = tiny_skia::Pixmap::new(256, 256).unwrap();
    for _ in 0..60 {
        let z = 10 + r.below(9) as u8;
        let n = (1u64 << z) as f64;
        let bx = &a.bounds;
        let x = ((bx.min_x + r.next() as f64 * (bx.max_x - bx.min_x)) * n) as u32;
        let y = ((bx.min_y + r.next() as f64 * (bx.max_y - bx.min_y)) * n) as u32;
        for theme in [ThemeName::Light, ThemeName::Dark] {
            let t = theme.theme();
            assert_eq!(
                crate::vtile::build(
                    &a,
                    t,
                    z.min(crate::vtile::MAX_ZOOM),
                    x >> z.saturating_sub(crate::vtile::MAX_ZOOM),
                    y >> z.saturating_sub(crate::vtile::MAX_ZOOM)
                ),
                crate::vtile::build(
                    &b,
                    t,
                    z.min(crate::vtile::MAX_ZOOM),
                    x >> z.saturating_sub(crate::vtile::MAX_ZOOM),
                    y >> z.saturating_sub(crate::vtile::MAX_ZOOM)
                ),
                "geo tile {z}/{x}/{y}"
            );
            for buildings in [Buildings::Flat, Buildings::Extruded] {
                let vp = Viewport::camera_tile(z, x as i64, y as i64, 1.0, 0.0, 0.0);
                let mut png = |m: &Map| {
                    pixmap.fill(tiny_skia::Color::TRANSPARENT);
                    Renderer::new(m, t, buildings).render_into(&vp, &mut pixmap);
                    crate::output::encode_png(&pixmap).unwrap()
                };
                assert_eq!(png(&a), png(&b), "tile {z}/{x}/{y}");
                tiles += 1;
                let (x0, y0) = (x as f64 / n - a.origin[0], y as f64 / n - a.origin[1]);
                let q = [
                    x0 as f32,
                    y0 as f32,
                    (x0 + 1.0 / n) as f32,
                    (y0 + 1.0 / n) as f32,
                ];
                a.query(q, z as f32, &mut ids);
                drawn += (ids.len() > 1) as usize;
            }
        }
    }
    eprintln!(
        "{tiles} raster tiles ({drawn} with features) and {} geo tiles identical",
        tiles / 2
    );
}
