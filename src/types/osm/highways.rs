use std::ffi::OsStr;

use flat_map::FlatMap;
use osmpbfreader::{OsmObj, OsmPbfReader};

use crate::traits::way::Way;

use super::node_id::NodeId;

pub struct Highway {
    pub id: i64,
    pub tags: FlatMap<String, String>,
    pub nodes: Vec<NodeId>,
}

impl Way for Highway {
    fn new(id: i64, tags: FlatMap<String, String>, nodes: Vec<NodeId>) -> Self {
        Highway { id, tags, nodes }
    }

    fn from_osm_way(way: osmpbfreader::Way) -> Highway {
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
        Highway::new(id, tags, nodes)
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

    fn from_osm_file(filename: &OsStr) ->  Vec<Self> {
        let r = std::fs::File::open(std::path::Path::new(filename)).unwrap();
        let mut pbf = OsmPbfReader::new(r);

        let mut highways: Vec<Highway> = Vec::new();

        for obj in pbf.par_iter().map(Result::unwrap) {
            match obj {
                OsmObj::Way(way) => {
                    if way.tags.get("highway").is_some() {
                        highways.push(Highway::from_osm_way(way));
                    }
                }
                _ => {}
            }
        }

        highways
    }
}

pub type Highways = Vec<Highway>;