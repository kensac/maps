//! Counts towers by type and prints a sample location of each.
fn main() {
    let path = std::env::args().nth(1).expect("usage: towers <pbf>");
    let map = maps::map::Map::build(maps::ingest::read(std::path::Path::new(&path)).unwrap());
    let mut seen = std::collections::BTreeMap::new();
    for f in &map.features {
        if f.kind != maps::classify::Kind::Tower {
            continue;
        }
        let p = map.ring(f.ring_start)[0];
        let (lon, lat) =
            maps::geo::unproject([p[0] as f64 + map.origin[0], p[1] as f64 + map.origin[1]]);
        let e = seen
            .entry(maps::classify::TowerType::from_u8(f.variant) as u8)
            .or_insert((0, lon, lat, f.base, f.height));
        e.0 += 1;
    }
    for (v, (n, lon, lat, base, top)) in seen {
        println!(
            "{:?} x{n}: e.g. {lon:.5},{lat:.5} base={base} top={top}",
            maps::classify::TowerType::from_u8(v)
        );
    }
}
