use flat_map::FlatMap;

use crate::types::osm::node_id::NodeId;

// Way trait to define all necessary functions for the various osm ways such as highways, waterways, etc.
// For now we need to define the following functions:
// - new
pub trait Way {
    fn new(id: i64, tags: FlatMap<String, String>, nodes: Vec<NodeId>) -> Self;
    fn from_osm_way(way: osmpbfreader::Way) -> Self;
    fn id(&self) -> i64;
    fn tags(&self) -> &FlatMap<String, String>;
    fn nodes(&self) -> &Vec<NodeId>;
    fn get_tag(&self, key: &str) -> Option<&String> {
        self.tags().get(key)
    }
}
