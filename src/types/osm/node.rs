use std::collections::HashSet;

use flat_map::FlatMap;

use super::node_id::NodeId;

#[derive(Debug, Clone)]
pub struct Node {
    pub id: NodeId,
    pub tags: FlatMap<String, String>,
    pub lat: i64, // lat and lon are multiplied by 10^7 to store them as i64
    pub lon: i64,
}

impl Node {
    pub fn new(id: NodeId, tags: FlatMap<String, String>, lat: i64, lon: i64) -> Self {
        Node { id, tags, lat, lon }
    }

    pub fn from_osm_node(node: osmpbfreader::Node) -> Node {
        let id = NodeId::new(node.id.0);
        let tags = node
            .tags
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Node::new(
            id,
            tags,
            (node.lat() * 10_000_000.0) as i64,
            (node.lon() * 10_000_000.0) as i64,
        )
    }

    pub fn id(&self) -> NodeId {
        self.id
    }

    pub fn tags(&self) -> &FlatMap<String, String> {
        &self.tags
    }

    pub fn lat_as_i64(&self) -> i64 {
        self.lat
    }

    pub fn lon_as_i64(&self) -> i64 {
        self.lon
    }

    pub fn lat(&self) -> f64 {
        self.lat as f64 / 10_000_000.0
    }

    pub fn lon(&self) -> f64 {
        self.lon as f64 / 10_000_000.0
    }

    pub fn get_tag(&self, key: &str) -> Option<&String> {
        self.tags().get(key)
    }
}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Node {}

impl std::hash::Hash for Node {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}


pub struct Nodes {
    pub nodes: HashSet<Node>,
}

impl Nodes {
    pub fn new(nodes: Vec<Node>) -> Self {
        Nodes {
            nodes: nodes.into_iter().collect(),
        }
    }

    pub fn from_osm_nodes(nodes: Vec<osmpbfreader::Node>) -> Nodes {
        let nodes = nodes
            .into_iter()
            .map(Node::from_osm_node)
            .collect();
        Nodes::new(nodes)
    }

    pub fn get_node_by_id(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&Node::new(id, FlatMap::new(), 0, 0))
    }


}

// Test for presence in Nodes of Node with id but different tags and coordinates
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_node_by_id() {
        let node1 = Node::new(NodeId::new(1), FlatMap::new(), 0, 0);
        let node2 = Node::new(NodeId::new(2), FlatMap::new(), 0, 0);
        let node3 = Node::new(NodeId::new(3), FlatMap::new(), 0, 0);
        let nodes = Nodes::new(vec![node1.clone(), node2.clone(), node3.clone()]);

        assert_eq!(nodes.get_node_by_id(NodeId::new(1)), Some(&node1));
        assert_eq!(nodes.get_node_by_id(NodeId::new(2)), Some(&node2));
        assert_eq!(nodes.get_node_by_id(NodeId::new(3)), Some(&node3));
        assert_eq!(nodes.get_node_by_id(NodeId::new(4)), None);
    }
}

