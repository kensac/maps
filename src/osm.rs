use std::{collections::HashMap, ffi::OsStr};

use osmpbfreader::{OsmObj, OsmPbfReader, Way};


pub fn read_osm_data(
    filename: &OsStr,
) -> (
    HashMap<i64, (f64, f64)>,
    Vec<(Vec<(f64, f64)>, u32)>,
    Vec<(Vec<(f64, f64)>, u32)>,
    Vec<(Vec<(f64, f64)>, u32)>,
) {
    let r = std::fs::File::open(&std::path::Path::new(filename)).unwrap();
    let mut pbf = OsmPbfReader::new(r);

    let mut nodes: HashMap<i64, (f64, f64)> = HashMap::new();
    let mut highways: Vec<(Vec<(f64, f64)>, u32)> = Vec::new();
    let mut waterways: Vec<(Vec<(f64, f64)>, u32)> = Vec::new();
    let mut railways: Vec<(Vec<(f64, f64)>, u32)> = Vec::new();

    for obj in pbf.par_iter().map(Result::unwrap) {
        match obj {
            OsmObj::Node(node) => {
                nodes.insert(node.id.0, (node.lon(), node.lat()));
            }
            OsmObj::Way(way) => {
                let way_nodes = extract_way_nodes(&way, &nodes);
                if way.tags.get("highway").is_some() {
                    highways.push(way_nodes);
                } else if way.tags.get("waterway").is_some() {
                    waterways.push(way_nodes);
                } else if way.tags.get("railway").is_some() {
                    railways.push(way_nodes);
                }
            }
            OsmObj::Relation(_) => {}
        }
    }
    (nodes, highways, waterways, railways)
}

pub fn extract_way_nodes(way: &Way, nodes: &HashMap<i64, (f64, f64)>) -> (Vec<(f64, f64)>, u32) {
    let nodes = way
        .nodes
        .iter()
        .filter_map(|node_id| nodes.get(&node_id.0))
        .map(|&(lon, lat)| (lon / 1e7, lat / 1e7)) // Convert from microdegrees to degrees
        .collect();

    let lanes = way
        .tags
        .get("lanes")
        .and_then(|width| width.parse::<u32>().ok())
        .unwrap_or(1);

    (nodes, lanes)
}
