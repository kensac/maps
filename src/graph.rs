use std::collections::HashMap;

use ordered_float::Pow;
use pathfinding::prelude::astar;
use crate::types::{Coord, Edge};

pub fn build_graph(highways: &[(Vec<(f64, f64)>, u32)]) -> HashMap<Coord, Vec<Edge>> {
    let mut graph = HashMap::new();
    for (way, _width) in highways {
        for window in way.windows(2) {
            let (lon1, lat1) = window[0];
            let (lon2, lat2) = window[1];
            let start_id = Coord { lat: lat1, lon: lon1 };
            let end_id = Coord { lat: lat2, lon: lon2 };

            let cost = haversine_distance(lon1, lat1, lon2, lat2);

            let source = ((lat1 * 1e7) as i64, (lon1 * 1e7) as i64);
            let target = ((lat2 * 1e7) as i64, (lon2 * 1e7) as i64);

            graph.entry(start_id).or_insert_with(Vec::new).push(Edge { target: end_id, cost });
            graph.entry(end_id).or_insert_with(Vec::new).push(Edge { target: start_id, cost });
        }
    }
    graph
}

fn haversine_distance(lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> i64 {
    let r = 6371e3; // Earth's radius in meters
    let phi1 = lat1.to_radians();
    let phi2 = lat2.to_radians();
    let delta_phi = (lat2 - lat1).to_radians();
    let delta_lambda = (lon2 - lon1).to_radians();

    let a = (delta_phi / 2.0).sin() * (delta_phi / 2.0).sin() +
            phi1.cos() * phi2.cos() *
            (delta_lambda / 2.0).sin() * (delta_lambda / 2.0).sin();
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    ((r * c) * 1e8 ) as i64
}


pub fn find_path(
    graph: &HashMap<Coord, Vec<Edge>>,
    start: Coord,
    goal: Coord,
) -> Option<(Vec<Coord>, i64)> {
    astar(
        &start,
        |&node| {
            graph.get(&node).unwrap_or(&vec![]).iter().map(|edge| {
                let cost = edge.cost;
                (edge.target, cost)
            }).collect::<Vec<_>>()
        },
        |&node| {
            haversine_distance(node.lon, node.lat, goal.lon, goal.lat)
        },
        |&node| node == goal
    ).map(|(path, cost)| (path, cost))
}

