use std::{collections::HashMap, ffi::OsStr};

use osmpbfreader::{OsmObj, OsmPbfReader, Way};

use crate::types::WayCoords;

pub fn read_osm_data(
    filename: &OsStr,
) -> (
    HashMap<i64, (f64, f64)>,
    Vec<WayCoords>,
    Vec<WayCoords>,
    Vec<WayCoords>,
    Vec<WayCoords>,
    Vec<WayCoords>,
) {

    // check if the file exists
    let r = std::fs::File::open(std::path::Path::new(filename)).unwrap();
    let mut pbf = OsmPbfReader::new(r);

    let mut nodes: HashMap<i64, (f64, f64)> = HashMap::new();
    let mut highways: Vec<WayCoords> = Vec::new();
    let mut waterways: Vec<WayCoords> = Vec::new();
    let mut railways: Vec<WayCoords> = Vec::new();
    let mut buildings: Vec<WayCoords> = Vec::new();
    let mut naturals: Vec<WayCoords> = Vec::new();

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
                } else if way.tags.get("building").is_some() {
                    buildings.push(way_nodes);
                } else if way.tags.get("natural").is_some() {
                    // natural=coastline is a special case
                    print!("{:?}", way.tags.get("natural"));
/*                      if way.tags.get("natural").unwrap() == "coastline" {
                        naturals.push(way_nodes);
                    }  */
                    naturals.push(way_nodes);
                }
            }
            OsmObj::Relation(_) => {}
        }
    }
    (nodes, highways, waterways, railways, buildings, naturals)
}

pub fn extract_way_nodes(way: &Way, nodes: &HashMap<i64, (f64, f64)>) -> WayCoords {
    way.nodes
        .iter()
        .filter_map(|node_id| nodes.get(&node_id.0))
        .map(|&(lon, lat)| (lon, lat))
        .collect()
}
