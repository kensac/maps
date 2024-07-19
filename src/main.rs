extern crate osmpbfreader;

use maps::cache::{load_cache, save_cache};
use maps::drawing::draw_map;
use maps::osm::read_osm_data;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;

#[derive(Serialize, Deserialize)]
pub struct CachedData {
    nodes: HashMap<i64, (f64, f64)>,
    highways: Vec<(Vec<(f64, f64)>, u32)>,
    waterways: Vec<(Vec<(f64, f64)>, u32)>,
    railways: Vec<(Vec<(f64, f64)>, u32)>,
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 2 {
        println!("usage: osm_visualizer filename");
        return;
    }
    let filename = &args[1];

    // Check if cache exists
    let cache_filename = format!("{}.cache", filename.to_str().unwrap());

    let start_time = Instant::now();

    let (nodes, highways, waterways, railways) =
        if let Ok(cached_data) = load_cache::<CachedData>(&cache_filename) {
            println!("Loaded data from cache.");
            (
                cached_data.nodes,
                cached_data.highways,
                cached_data.waterways,
                cached_data.railways,
            )
        } else {
            println!("Parsing OSM data.");
            let parse_start_time = Instant::now();
            let parsed_data = read_osm_data(filename);
            let parse_duration = parse_start_time.elapsed();
            println!("OSM data parsed in {:?}", parse_duration);

            let save_start_time = Instant::now();
            save_cache(&cache_filename, &parsed_data).expect("Failed to save cache.");
            let save_duration = save_start_time.elapsed();
            println!("Cache saved in {:?}", save_duration);

            parsed_data
        };

    let cache_or_parse_duration = start_time.elapsed();
    println!(
        "Cache load/parse completed in {:?}",
        cache_or_parse_duration
    );

    let draw_start_time = Instant::now();
    // Draw the map
    draw_map(&highways, &waterways, &railways);
    let draw_duration = draw_start_time.elapsed();
    println!("Map drawn in {:?}", draw_duration);

    let total_duration = start_time.elapsed();
    println!("Total execution time: {:?}", total_duration);
}
