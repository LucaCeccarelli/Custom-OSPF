use std::collections::{HashMap, VecDeque};

pub fn compute_routes(
    graph: &HashMap<String, Vec<String>>,
    source: &str,
) -> HashMap<String, Vec<String>> {
    let mut routes = HashMap::new();
    let mut queue = VecDeque::new();
    let mut visited = HashMap::new();

    queue.push_back((source.to_string(), vec![source.to_string()]));

    while let Some((node, path)) = queue.pop_front() {
        for neighbor in graph.get(&node).unwrap_or(&vec![]) {
            if !visited.contains_key(neighbor) {
                let mut new_path = path.clone();
                new_path.push(neighbor.clone());
                visited.insert(neighbor.clone(), new_path.clone());
                queue.push_back((neighbor.clone(), new_path));
            }
        }
    }

    routes.extend(visited);
    routes
}
