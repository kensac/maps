use std::ffi::OsStr;

use flat_map::FlatMap;
use osmpbfreader::{OsmObj, OsmPbfReader};

use crate::types::osm::node_id::NodeId;

// Way trait to define all necessary functions for the various osm ways such as highways, waterways, etc.
// For now we need to define the following functions:
// - Method to read that way from the osm file
pub trait Way {
    fn new(id: i64, tags: FlatMap<String, String>, nodes: Vec<NodeId>) -> Self;
    fn from_osm_way(way: osmpbfreader::Way) -> Self;
    fn id(&self) -> i64;
    fn tags(&self) -> &FlatMap<String, String>;
    fn nodes(&self) -> &Vec<NodeId>;
    fn get_tag(&self, key: &str) -> Option<&String> {
        self.tags().get(key)
    }
    fn from_osm_file(filename: &OsStr) -> Vec<Self> where Self: Sized;
}

/*     let r = std::fs::File::open(std::path::Path::new(filename)).unwrap();
let mut pbf = OsmPbfReader::new(r);

let mut highways: Vec<WayCoords> = Vec::new();

for obj in pbf.par_iter().map(Result::unwrap) {
    match obj {
        OsmObj::Way(way) => {
            ways.insert(way.id.0, way.clone());
                highways.push(way_nodes);
        }
    }
} */
