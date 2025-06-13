use std::collections::{HashMap, BinaryHeap};
use std::cmp::Ordering;
use crate::{RouterId, NetworkId};
use crate::network::Topology;

#[derive(Debug, Clone)]
pub struct ShortestPath {
    pub cost: u32,
    pub next_hop: Option<RouterId>,
    pub path: Vec<RouterId>,
}

#[derive(Debug)]
struct State {
    cost: u32,
    router: RouterId,
    path: Vec<RouterId>,
}

impl Eq for State {}

impl PartialEq for State {
    fn eq(&self, other: &Self) -> bool {
        self.cost == other.cost
    }
}

impl Ord for State {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap
        other.cost.cmp(&self.cost)
    }
}

impl PartialOrd for State {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub fn calculate_shortest_paths(topology: &Topology, source: &str) -> HashMap<NetworkId, ShortestPath> {
    let mut distances: HashMap<RouterId, u32> = HashMap::new();
    let mut previous: HashMap<RouterId, Option<RouterId>> = HashMap::new();
    let mut heap = BinaryHeap::new();

    // Initialize distances
    for router_id in topology.routers.keys() {
        distances.insert(router_id.clone(), u32::MAX);
        previous.insert(router_id.clone(), None);
    }
    distances.insert(source.to_string(), 0);

    heap.push(State {
        cost: 0,
        router: source.to_string(),
        path: vec![source.to_string()],
    });

    // Dijkstra's algorithm
    while let Some(State { cost, router, path }) = heap.pop() {
        // Skip if we've already found a better path
        if cost > *distances.get(&router).unwrap_or(&u32::MAX) {
            continue;
        }

        // Check all neighbors
        for (neighbor, link_cost) in topology.get_neighbors(&router) {
            let new_cost = cost + link_cost;

            if new_cost < *distances.get(&neighbor).unwrap_or(&u32::MAX) {
                distances.insert(neighbor.clone(), new_cost);
                previous.insert(neighbor.clone(), Some(router.clone()));

                let mut new_path = path.clone();
                new_path.push(neighbor.clone());

                heap.push(State {
                    cost: new_cost,
                    router: neighbor,
                    path: new_path,
                });
            }
        }
    }

    // Convert router-to-router paths to network paths
    let mut network_paths = HashMap::new();

    for network in topology.get_all_networks() {
        // Find which router owns this network
        if let Some((dest_router, _)) = topology.routers
            .iter()
            .find(|(_, info)| info.networks.contains(&network)) {

            if let Some(&cost) = distances.get(dest_router) {
                if cost < u32::MAX {
                    let next_hop = find_next_hop(&previous, source, dest_router);
                    let path = reconstruct_path(&previous, dest_router);

                    network_paths.insert(network, ShortestPath {
                        cost,
                        next_hop,
                        path,
                    });
                }
            }
        }
    }

    network_paths
}

fn find_next_hop(previous: &HashMap<RouterId, Option<RouterId>>, source: &str, dest: &str) -> Option<RouterId> {
    if dest == source {
        return None;
    }

    let mut current = dest;
    loop {
        if let Some(Some(prev)) = previous.get(current) {
            if prev == source {
                return Some(current.to_string());
            }
            current = prev;
        } else {
            break;
        }
    }
    None
}

fn reconstruct_path(previous: &HashMap<RouterId, Option<RouterId>>, dest: &str) -> Vec<RouterId> {
    let mut path = Vec::new();
    let mut current = Some(dest.to_string());

    while let Some(router) = current {
        path.push(router.clone());
        current = previous.get(&router).and_then(|p| p.clone());
    }

    path.reverse();
    path
}
