extern crate osmpbfreader;
extern crate rand;

use maps::cache::{self, load_cache, save_cache};
use maps::drawing::draw_map;
use maps::graph::find_path;
use maps::osm::read_osm_data;
use maps::types::{CachedData, Coord};
use rand::seq::IteratorRandom;
use std::time::Instant;

fn get_random_node(highways: &[(Vec<(f64, f64)>, u32)]) -> (f64, f64) {
    let mut rng = rand::thread_rng();
    let random_highway = highways.iter().choose(&mut rng);
    let random_node = random_highway.unwrap().0.iter().choose(&mut rng);
    *random_node.unwrap()
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

    let (_nodes, highways, waterways, railways, buildings, graph) =
        if let Ok(cached_data) = load_cache::<CachedData>(&cache_filename) {
            println!("Loaded data from cache.");
            (
                cached_data.nodes,
                cached_data.highways,
                cached_data.waterways,
                cached_data.railways,
                cached_data.buildings,
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
                graph,
            );

            save_cache(&cache_filename, &return_data).expect("Failed to save cache.");
            let save_duration = save_start_time.elapsed();
            println!("Cache saved in {:?}", save_duration);
            return_data
        };

    let cache_or_parse_duration = start_time.elapsed();
    println!(
        "Cache load/parse completed in {:?}",
        cache_or_parse_duration
    );

    let a_star_start_time = Instant::now();
    // pick a random node from nodes as start and goal
    let start_random = get_random_node(&highways);
    let goal_random = get_random_node(&highways);
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
        "Some nodes in graph: {:?}",
        graph.iter().take(5).collect::<Vec<_>>()
    );
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

    let draw_start_time = Instant::now();
    // Draw the map
    draw_map(
        &highways,
        &waterways,
        &railways,
        &buildings,
        &path_result
            .iter()
            .map(|&coord| (coord.lon, coord.lat))
            .collect::<Vec<_>>(),
            
    );
    let draw_duration = draw_start_time.elapsed();
    println!("Map drawn in {:?}", draw_duration);

    let total_duration = start_time.elapsed();
    println!("Total execution time: {:?}", total_duration);
}
