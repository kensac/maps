use super::coord::Coord;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct Edge {
    pub target: Coord, // (lat, lon)
    pub cost: i64,
}
