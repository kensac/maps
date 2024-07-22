use flat_map::FlatMap;

use crate::traits::way::Way;

use super::node_id::NodeId;

pub struct Highways {
    pub id: i64,
    pub tags: FlatMap<String, String>,
    pub nodes: Vec<NodeId>,
}

impl Way for Highways {
    fn new(id: i64, tags: FlatMap<String, String>, nodes: Vec<NodeId>) -> Self {
        Highways { id, tags, nodes }
    }

    fn from_osm_way(way: osmpbfreader::Way) -> Highways {
        let id = way.id.0;
        let tags = way
            .tags
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let nodes = way
            .nodes
            .iter()
            .map(|node_id| NodeId::new(node_id.0))
            .collect();
        Highways::new(id, tags, nodes)
    }

    fn id(&self) -> i64 {
        self.id
    }

    fn tags(&self) -> &FlatMap<String, String> {
        &self.tags
    }

    fn nodes(&self) -> &Vec<NodeId> {
        &self.nodes
    }
}
