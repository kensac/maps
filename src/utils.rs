use rand::seq::IteratorRandom;

pub fn get_random_node(highways: &[Vec<(f64, f64)>]) -> (f64, f64) {
    let mut rng = rand::thread_rng();
    let random_highway = highways.iter().choose(&mut rng);
    let random_node = random_highway.unwrap().iter().choose(&mut rng);
    *random_node.unwrap()
}
