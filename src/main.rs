extern crate osmpbfreader;
extern crate rand;

use maps::cache::{load_cache, save_cache};
use maps::drawing::draw_map;
use maps::graph::find_path;
use maps::osm::read_osm_data;
use maps::types::{CachedData, Coord, Edge, WayCoords};
use maps::utils::get_random_node;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::time::Instant;

fn main() {
    // set RUST_BACKTRACE=1 to see backtrace
    std::env::set_var("RUST_BACKTRACE", "1");
    let start_time = Instant::now();

    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 2 {
        return;
    }
    let filename = &args[1];

    // Check if cache exists
    let (_nodes, highways, waterways, railways, buildings, naturals, aeroways, multipolyons, graph) =
        load_or_parse_data(filename);

    // Run A* search
    let path_result = run_a_star(&highways, graph);
    let path_result_f64 = path_result
        .iter()
        .map(|&coord| (coord.lon, coord.lat))
        .collect::<Vec<_>>();

    // Draw map
    let draw_start_time = Instant::now();
    draw_map(
        &highways,
        &waterways,
        &railways,
        &buildings,
        &naturals,
        &aeroways,
        &multipolyons,
        &path_result_f64,
    );
    let draw_duration = draw_start_time.elapsed();
    println!("Map drawn in {:?}", draw_duration);

    let total_duration = start_time.elapsed();
    println!("Total execution time: {:?}", total_duration);
}

fn load_or_parse_data(
    filename: &OsStr,
) -> (
    HashMap<i64, (f64, f64)>,
    Vec<WayCoords>,
    Vec<WayCoords>,
    Vec<WayCoords>,
    Vec<WayCoords>,
    Vec<WayCoords>,
    Vec<WayCoords>,
    Vec<Vec<WayCoords>>,
    HashMap<Coord, Vec<Edge>>,
) {
    let start_time = Instant::now();
    let cache_filename = format!("{}.cache", filename.to_str().unwrap());
    let data = if let Ok(cached_data) = load_cache::<CachedData>(&cache_filename) {
        println!("Loaded data from cache.");
        println!("Loaded data in {:?}", start_time.elapsed());
        (
            cached_data.nodes,
            cached_data.highways,
            cached_data.waterways,
            cached_data.railways,
            cached_data.buildings,
            cached_data.naturals,
            cached_data.aeroways,
            cached_data.multipolygons,
            cached_data.graph,
        )
    } else {
        println!("Parsing OSM data.");
        let parse_start_time = Instant::now();
        let parsed_data = read_osm_data(filename);
        let parse_duration = parse_start_time.elapsed();
        println!("OSM data parsed in {:?}", parse_duration);

        let build_graph_start_time = Instant::now();
        let graph = maps::graph::build_graph(&parsed_data.1);
        let build_graph_duration = build_graph_start_time.elapsed();
        println!("Graph built in {:?}", build_graph_duration);

        let save_start_time = Instant::now();
        let return_data = (
            parsed_data.0,
            parsed_data.1,
            parsed_data.2,
            parsed_data.3,
            parsed_data.4,
            parsed_data.5,
            parsed_data.6,
            parsed_data.7,
            graph,
        );

        save_cache(OsStr::new(&cache_filename), &return_data).expect("Failed to save cache.");
        let save_duration = save_start_time.elapsed();
        println!("Cache saved in {:?}", save_duration);
        return_data
    };

    let cache_or_parse_duration = start_time.elapsed();
    println!(
        "Cache load/parse completed in {:?}",
        cache_or_parse_duration
    );

    data
}

fn run_a_star(highways: &[WayCoords], graph: HashMap<Coord, Vec<Edge>>) -> Vec<Coord> {
    let a_star_start_time = Instant::now();
    // pick a random node from nodes as start and goal

    let start_random = get_random_node(highways);
    let goal_random = get_random_node(highways);
    let start_random_coord = Coord {
        lat: start_random.1,
        lon: start_random.0,
    };

    let goal_random_coord = Coord {
        lat: goal_random.1,
        lon: goal_random.0,
    };

    println!("Start: {:?}", start_random_coord);
    println!("Goal: {:?}", goal_random_coord);
    println!("Graph size: {}", graph.len());
    println!(
        "nodes in graph with start: {:?}",
        graph.get(&start_random_coord).unwrap().len()
    );

    let path_result;
    if let Some((result, cost)) = find_path(&graph, start_random_coord, goal_random_coord) {
        println!("Path found with cost {}", cost);
        path_result = result;
    } else {
        println!("No path found.");
        path_result = vec![];
    }
    let a_star_duration = a_star_start_time.elapsed();
    println!("A* search completed in {:?}", a_star_duration);

    path_result
}
