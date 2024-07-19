extern crate osmpbfreader;

use osmpbfreader::{NodeId, OsmObj, OsmPbfReader};
use plotters::prelude::*;
use std::collections::HashMap;

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 2 {
        println!("usage: osm_visualizer filename");
        return;
    }
    let filename = &args[1];

    // Read OSM data
    let (nodes, ways) = read_osm_data(filename);

    // Draw the map
    draw_map(&ways);
}

fn read_osm_data(
    filename: &std::ffi::OsStr,
) -> (HashMap<NodeId, (f64, f64)>, Vec<Vec<(f64, f64)>>) {
    let r = std::fs::File::open(&std::path::Path::new(filename)).unwrap();
    let mut pbf = OsmPbfReader::new(r);

    let mut nodes: HashMap<NodeId, (f64, f64)> = HashMap::new();
    let mut ways: Vec<Vec<(f64, f64)>> = Vec::new();

    for obj in pbf.par_iter().map(Result::unwrap) {
        match obj {
            OsmObj::Node(node) => {
                nodes.insert(node.id, (node.lon(), node.lat()));
            }
            OsmObj::Way(way) => {
                let mut way_nodes = Vec::new();
                for node_id in way.nodes {
                    if let Some(&(lon, lat)) = nodes.get(&node_id) {
                        // Convert from microdegrees to degrees
                        way_nodes.push((lon as f64 / 1e7, lat as f64 / 1e7));
                    }
                }
                ways.push(way_nodes);
            }
            OsmObj::Relation(_) => {}
        }
    }
    (nodes, ways)
}

fn draw_map(ways: &Vec<Vec<(f64, f64)>>) {
    if ways.is_empty() {
        println!("No ways to draw.");
        return;
    }

    // Calculate bounding box
    let (mut min_lon, mut min_lat) = (std::f64::MAX, std::f64::MAX);
    let (mut max_lon, mut max_lat) = (std::f64::MIN, std::f64::MIN);

    for way in ways {
        for &(lon, lat) in way {
            if lon < min_lon {
                min_lon = lon;
            }
            if lon > max_lon {
                max_lon = lon;
            }
            if lat < min_lat {
                min_lat = lat;
            }
            if lat > max_lat {
                max_lat = lat;
            }
        }
    }

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

    for way in ways {
        chart
            .draw_series(LineSeries::new(way.clone(), &RED))
            .unwrap();
    }

    root.present().unwrap();
}
