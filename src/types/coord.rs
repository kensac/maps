use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct Coord {
    pub lon: f64,
    pub lat: f64,
}

impl Coord {
    pub fn new(lon: f64, lat: f64) -> Self {
        Coord { lon, lat }
    }
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
