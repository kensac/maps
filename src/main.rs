extern crate osmpbfreader;

use osmpbfreader::{OsmObj, OsmPbfReader, Way};
use plotters::coord::types::RangedCoordf64;
use plotters::prelude::*;
use std::collections::HashMap;
use std::ffi::OsStr;

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 2 {
        println!("usage: osm_visualizer filename");
        return;
    }
    let filename = &args[1];

    // Read OSM data
    let (nodes, highways, waterways, railways) = read_osm_data(filename);

    // Draw the map
    draw_map(&highways, &waterways, &railways);
}

fn read_osm_data(
    filename: &OsStr,
) -> (
    HashMap<i64, (f64, f64)>,
    Vec<Vec<(f64, f64)>>,
    Vec<Vec<(f64, f64)>>,
    Vec<Vec<(f64, f64)>>,
) {
    let r = std::fs::File::open(&std::path::Path::new(filename)).unwrap();
    let mut pbf = OsmPbfReader::new(r);

    let mut nodes: HashMap<i64, (f64, f64)> = HashMap::new();
    let mut highways: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut waterways: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut railways: Vec<Vec<(f64, f64)>> = Vec::new();

    for obj in pbf.par_iter().map(Result::unwrap) {
        match obj {
            OsmObj::Node(node) => {
                nodes.insert(node.id.0, (node.lon(), node.lat()));
            }
            OsmObj::Way(way) => {
                let way_nodes = extract_way_nodes(&way, &nodes);
                match way.tags.get("highway") {
                    Some(_) => highways.push(way_nodes),
                    None => match way.tags.get("waterway") {
                        Some(_) => waterways.push(way_nodes),
                        None => match way.tags.get("railway") {
                            Some(_) => railways.push(way_nodes),
                            None => {}
                        },
                    },
                }
            }
            OsmObj::Relation(_) => {}
        }
    }
    (nodes, highways, waterways, railways)
}

fn extract_way_nodes(way: &Way, nodes: &HashMap<i64, (f64, f64)>) -> Vec<(f64, f64)> {
    way.nodes
        .iter()
        .filter_map(|node_id| nodes.get(&node_id.0))
        .map(|&(lon, lat)| (lon / 1e7, lat / 1e7)) // Convert from microdegrees to degrees
        .collect()
}

fn draw_map(
    highways: &[Vec<(f64, f64)>],
    waterways: &[Vec<(f64, f64)>],
    railways: &[Vec<(f64, f64)>],
) {
    if highways.is_empty() && waterways.is_empty() && railways.is_empty() {
        println!("No ways to draw.");
        return;
    }

    // Calculate bounding box
    let (min_lon, min_lat, max_lon, max_lat) =
        calculate_bounding_box(&[highways, waterways, railways]);

    println!(
        "Bounding box: ({}, {}), ({}, {})",
        min_lon, min_lat, max_lon, max_lat
    );

    let root = BitMapBackend::new("osm_map.png", (4096 * 4 * 2, 3072 * 4 * 2)).into_drawing_area();
    root.fill(&WHITE).unwrap();

    let mut chart = ChartBuilder::on(&root)
        .caption("OSM Map", ("sans-serif", 50).into_font())
        .margin(10)
        .x_label_area_size(30)
        .y_label_area_size(30)
        .build_cartesian_2d(min_lon..max_lon, min_lat..max_lat)
        .unwrap();

    chart.configure_mesh().draw().unwrap();

    draw_way(&mut chart, highways, BLACK);
    draw_way(&mut chart, waterways, BLUE);
    draw_way(&mut chart, railways, RED);

    root.present().unwrap();
}

fn draw_way(
    chart: &mut ChartContext<BitMapBackend, Cartesian2d<RangedCoordf64, RangedCoordf64>>,
    ways: &[Vec<(f64, f64)>],
    color: RGBColor,
) {
    for way in ways {
        chart
            .draw_series(LineSeries::new(way.iter().cloned(), &color))
            .unwrap();
    }
}

fn calculate_bounding_box(series: &[&[Vec<(f64, f64)>]]) -> (f64, f64, f64, f64) {
    let (mut min_lon, mut min_lat) = (f64::INFINITY, f64::INFINITY);
    let (mut max_lon, mut max_lat) = (f64::NEG_INFINITY, f64::NEG_INFINITY);

    for ways in series {
        for way in *ways {
            for &(lon, lat) in way {
                min_lon = min_lon.min(lon);
                max_lon = max_lon.max(lon);
                min_lat = min_lat.min(lat);
                max_lat = max_lat.max(lat);
            }
        }
    }

    (min_lon, min_lat, max_lon, max_lat)
}
