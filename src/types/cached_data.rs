use super::coord::Coord;
use super::edge::Edge;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize)]
pub struct CachedData {
    pub nodes: HashMap<i64, (f64, f64)>,
    pub highways: Vec<WayCoords>,
    pub waterways: Vec<WayCoords>,
    pub railways: Vec<WayCoords>,
    pub buildings: Vec<WayCoords>,
    pub naturals: Vec<WayCoords>,
    pub aeroways: Vec<WayCoords>,
    pub multipolygons: Vec<Vec<WayCoords>>,
    pub graph: HashMap<Coord, Vec<Edge>>,
}

pub type WayCoords = Vec<(f64, f64)>;
