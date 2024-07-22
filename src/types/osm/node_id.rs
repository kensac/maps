use std::os;

#[derive(Debug, Clone, Copy, Hash)]
pub struct NodeId(pub i64);

impl NodeId {
    pub fn new(id: i64) -> Self {
        NodeId(id)
    }
}

impl PartialEq for NodeId {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for NodeId {}

impl From<i64> for NodeId {
    fn from(id: i64) -> Self {
        NodeId(id)
    }
}

impl From<osmpbfreader::NodeId> for NodeId {
    fn from(id: osmpbfreader::NodeId) -> Self {
        NodeId(id.0)
    }
}
