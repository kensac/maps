//! Lists solids near a point: `cargo run --release --example near -- <pbf> <lon> <lat> [radius_m]`.
use maps::geo::{meters_per_unit, project, unproject};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (lon, lat): (f64, f64) = (a[2].parse().unwrap(), a[3].parse().unwrap());
    let radius: f64 = a.get(4).map_or(60.0, |r| r.parse().unwrap());
    let map = maps::map::Map::build(maps::ingest::read(std::path::Path::new(&a[1])).unwrap());
    let p = project(lon, lat);
    let mpu = meters_per_unit(p[1]);
    for f in &map.features {
        if !(f.kind.is_extrusion() || f.kind.is_point()) {
            continue;
        }
        let cx = (f.bbox[0] + f.bbox[2]) as f64 / 2.0 + map.origin[0];
        let cy = (f.bbox[1] + f.bbox[3]) as f64 / 2.0 + map.origin[1];
        let d = ((cx - p[0]).powi(2) + (cy - p[1]).powi(2)).sqrt() * mpu;
        if d > radius {
            continue;
        }
        let det = map.detail(f);
        let (w, h) = (
            (f.bbox[2] - f.bbox[0]) as f64 * mpu,
            (f.bbox[3] - f.bbox[1]) as f64 * mpu,
        );
        let (clon, clat) = unproject([cx, cy]);
        println!("{:?} flags={} base={} top={} roof={:?} rh={} vis={} {w:.0}x{h:.0}m @{clon:.5},{clat:.5}", f.kind, f.flags, f.base, f.height, det.roof, det.roof_height, f.vis_zoom);
    }
}
