use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct CachedData {
    pub nodes: HashMap<i64, (f64, f64)>,
    pub highways: Vec<(Vec<(f64, f64)>, u32)>,
    pub waterways: Vec<(Vec<(f64, f64)>, u32)>,
    pub railways: Vec<(Vec<(f64, f64)>, u32)>,
    pub graph: HashMap<Coord, Vec<Edge>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct Coord {
    pub lon: f64,
    pub lat: f64,
}

impl PartialEq for Coord {
    fn eq(&self, other: &Self) -> bool {
        self.lat == other.lat && self.lon == other.lon
    }
}

impl Eq for Coord {}

impl Hash for Coord {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.lat.to_bits().hash(state);
        self.lon.to_bits().hash(state);
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Edge {
    pub target: Coord, // (lat, lon)
    pub cost: i64,
}
