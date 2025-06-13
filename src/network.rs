use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use crate::message::{LSAMessage, LinkData};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkNode {
    pub router_id: String,
    pub router_name: String,
    pub links: Vec<LinkData>,
    pub is_default_router: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkEdge {
    pub from: String,
    pub to: String,
    pub cost: u32,
    pub bandwidth: u64,
    pub available_bandwidth: u64,
    pub active: bool,
}

#[derive(Debug)]
pub struct NetworkTopology {
    pub nodes: HashMap<String, NetworkNode>,
    pub edges: Vec<NetworkEdge>,
    pub sequence_number: u32,
}

#[derive(Debug, Clone)]
pub struct ShortestPath {
    pub destination: String,
    pub next_hop: Option<String>,
    pub cost: u32,
    pub path: Vec<String>,
    pub bandwidth: u64,
}

impl NetworkTopology {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: Vec::new(),
            sequence_number: 0,
        }
    }

    pub fn add_node(&mut self, node: NetworkNode) {
        self.nodes.insert(node.router_id.clone(), node);
        self.sequence_number += 1;
    }

    pub fn remove_node(&mut self, router_id: &str) {
        self.nodes.remove(router_id);
        self.edges.retain(|edge| edge.from != router_id && edge.to != router_id);
        self.sequence_number += 1;
    }

    pub fn update_from_lsa(&mut self, lsa: &LSAMessage) {
        match &lsa.data {
            crate::message::LSAData::Router(router_data) => {
                let node = NetworkNode {
                    router_id: lsa.advertising_router.clone(),
                    router_name: lsa.advertising_router.clone(), // Would be set elsewhere
                    links: router_data.links.clone(),
                    is_default_router: false, // Would be determined elsewhere
                };

                // Remove old edges from this router
                self.edges.retain(|edge| edge.from != lsa.advertising_router);

                // Add new edges
                for link in &router_data.links {
                    let edge = NetworkEdge {
                        from: lsa.advertising_router.clone(),
                        to: link.link_id.clone(),
                        cost: link.metric,
                        bandwidth: link.bandwidth,
                        available_bandwidth: link.available_bandwidth,
                        active: true,
                    };
                    self.edges.push(edge);
                }

                self.add_node(node);
            }
            _ => {} // Handle other LSA types if needed
        }
    }

    pub fn calculate_shortest_paths(&self, source: &str) -> HashMap<String, ShortestPath> {
        // Dijkstra's algorithm implementation
        let mut distances: HashMap<String, u32> = HashMap::new();
        let mut previous: HashMap<String, String> = HashMap::new();
        let mut unvisited: std::collections::BinaryHeap<std::cmp::Reverse<(u32, String)>> =
            std::collections::BinaryHeap::new();

        // Initialize distances
        for router_id in self.nodes.keys() {
            if router_id == source {
                distances.insert(router_id.clone(), 0);
                unvisited.push(std::cmp::Reverse((0, router_id.clone())));
            } else {
                distances.insert(router_id.clone(), u32::MAX);
                unvisited.push(std::cmp::Reverse((u32::MAX, router_id.clone())));
            }
        }

        while let Some(std::cmp::Reverse((current_distance, current_node))) = unvisited.pop() {
            if current_distance > distances[&current_node] {
                continue;
            }

            // Check all neighbors
            for edge in &self.edges {
                if edge.from == current_node && edge.active {
                    let neighbor = &edge.to;
                    let tentative_distance = current_distance.saturating_add(edge.cost);

                    if tentative_distance < distances[neighbor] {
                        distances.insert(neighbor.clone(), tentative_distance);
                        previous.insert(neighbor.clone(), current_node.clone());
                        unvisited.push(std::cmp::Reverse((tentative_distance, neighbor.clone())));
                    }
                }
            }
        }

        // Build shortest paths
        let mut paths = HashMap::new();

        for (destination, &cost) in &distances {
            if destination != source && cost != u32::MAX {
                let mut path = Vec::new();
                let mut current = destination.clone();

                // Reconstruct path
                while let Some(prev) = previous.get(&current) {
                    path.push(current.clone());
                    current = prev.clone();
                }
                path.push(source.to_string());
                path.reverse();

                let next_hop = if path.len() > 1 {
                    Some(path[1].clone())
                } else {
                    None
                };

                // Calculate minimum bandwidth along path
                let bandwidth = self.calculate_path_bandwidth(&path);

                paths.insert(destination.clone(), ShortestPath {
                    destination: destination.clone(),
                    next_hop,
                    cost,
                    path,
                    bandwidth,
                });
            }
        }

        paths
    }

    fn calculate_path_bandwidth(&self, path: &[String]) -> u64 {
        let mut min_bandwidth = u64::MAX;

        for i in 0..path.len().saturating_sub(1) {
            let from = &path[i];
            let to = &path[i + 1];

            if let Some(edge) = self.edges.iter().find(|e| e.from == *from && e.to == *to) {
                min_bandwidth = min_bandwidth.min(edge.available_bandwidth);
            }
        }

        if min_bandwidth == u64::MAX {
            0
        } else {
            min_bandwidth
        }
    }

    pub fn get_neighbors(&self, router_id: &str) -> Vec<String> {
        self.edges
            .iter()
            .filter(|edge| edge.from == router_id && edge.active)
            .map(|edge| edge.to.clone())
            .collect()
    }

    pub fn mark_link_down(&mut self, from: &str, to: &str) {
        for edge in &mut self.edges {
            if (edge.from == from && edge.to == to) || (edge.from == to && edge.to == from) {
                edge.active = false;
            }
        }
        self.sequence_number += 1;
    }

    pub fn mark_link_up(&mut self, from: &str, to: &str) {
        for edge in &mut self.edges {
            if (edge.from == from && edge.to == to) || (edge.from == to && edge.to == from) {
                edge.active = true;
            }
        }
        self.sequence_number += 1;
    }

    pub fn update_bandwidth(&mut self, from: &str, to: &str, available_bandwidth: u64) {
        for edge in &mut self.edges {
            if edge.from == from && edge.to == to {
                edge.available_bandwidth = available_bandwidth;
            }
        }
        self.sequence_number += 1;
    }

    pub fn get_default_router(&self) -> Option<&NetworkNode> {
        self.nodes.values().find(|node| node.is_default_router)
    }

    pub fn set_default_router(&mut self, router_id: &str) {
        // Clear previous default router
        for node in self.nodes.values_mut() {
            node.is_default_router = false;
        }

        // Set new default router
        if let Some(node) = self.nodes.get_mut(router_id) {
            node.is_default_router = true;
        }

        self.sequence_number += 1;
    }
}