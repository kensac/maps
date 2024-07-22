use std::{collections::HashMap, ffi::OsStr};

use osmpbfreader::{OsmId, OsmObj, OsmPbfReader, Relation, Way};

use crate::types::cached_data::WayCoords;

pub fn read_osm_data(
    filename: &OsStr,
) -> (
    HashMap<i64, (f64, f64)>,
    Vec<WayCoords>,
    Vec<WayCoords>,
    Vec<WayCoords>,
    Vec<WayCoords>,
    Vec<WayCoords>,
    Vec<WayCoords>,
    Vec<Vec<WayCoords>>, // Multipolygons
) {
    // Check if the file exists
    let r = std::fs::File::open(std::path::Path::new(filename)).unwrap();
    let mut pbf = OsmPbfReader::new(r);

    let mut nodes: HashMap<i64, (f64, f64)> = HashMap::new();
    let mut highways: Vec<WayCoords> = Vec::new();
    let mut waterways: Vec<WayCoords> = Vec::new();
    let mut railways: Vec<WayCoords> = Vec::new();
    let mut buildings: Vec<WayCoords> = Vec::new();
    let mut naturals: Vec<WayCoords> = Vec::new();
    let mut aeroways: Vec<WayCoords> = Vec::new();
    let mut multipolygons: Vec<Vec<WayCoords>> = Vec::new();

    let mut ways: HashMap<i64, Way> = HashMap::new();
    let mut relations: Vec<Relation> = Vec::new();

    for obj in pbf.par_iter().map(Result::unwrap) {
        match obj {
            OsmObj::Node(node) => {
                nodes.insert(node.id.0, (node.lon(), node.lat()));
            }
            OsmObj::Way(way) => {
                ways.insert(way.id.0, way.clone());
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
                    naturals.push(way_nodes);
                } else if way.tags.get("aeroway").is_some() {
                    aeroways.push(way_nodes);
                }
            }
            OsmObj::Relation(relation) => {
                relations.push(relation);
            }
        }
    }

    for relation in relations {
        if relation.tags.get("type") == Some(&smartstring::alias::String::from("multipolygon")) {
            let mut multipolygon_ways: Vec<WayCoords> = Vec::new();
            for member in &relation.refs {
                if let OsmId::Way(id) = member.member {
                    if let Some(way) = ways.get(&id.0) {
                        multipolygon_ways.push(extract_way_nodes(way, &nodes));
                    }
                }
            }
            multipolygons.push(multipolygon_ways);
        }
    }

    (
        nodes,
        highways,
        waterways,
        railways,
        buildings,
        naturals,
        aeroways,
        multipolygons,
    )
}

pub fn extract_way_nodes(way: &Way, nodes: &HashMap<i64, (f64, f64)>) -> WayCoords {
    way.nodes
        .iter()
        .filter_map(|node_id| nodes.get(&node_id.0))
        .map(|&(lon, lat)| (lon, lat))
        .collect()
}
