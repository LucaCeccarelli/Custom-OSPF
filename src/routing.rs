use crate::types::*;
use std::collections::{HashMap, BinaryHeap};
use std::cmp::Reverse;
use std::net::IpAddr;

pub struct RoutingEngine {
    topology: NetworkTopology,
    local_router: IpAddr,
}

impl RoutingEngine {
    pub fn new(local_router: IpAddr) -> Self {
        RoutingEngine {
            topology: NetworkTopology {
                routers: HashMap::new(),
                links: Vec::new(),
                routes: HashMap::new(),
            },
            local_router,
        }
    }

    pub fn update_topology(&mut self, message: RoutingMessage) {
        // Update router information
        let router = Router {
            id: message.sender,
            hostname: message.neighbors.iter()
                .find(|n| n.id == message.sender)
                .map(|n| n.hostname.clone())
                .unwrap_or_else(|| message.sender.to_string()),
            last_seen: message.timestamp,
            active: true,
        };

        self.topology.routers.insert(message.sender, router);

        // Update neighbor information
        for neighbor in message.neighbors {
            self.topology.routers.insert(neighbor.id, neighbor);
        }

        // Update routes from this router
        for route in message.routes {
            self.topology.routes.insert(route.destination, route);
        }
    }

    pub fn compute_shortest_paths(&mut self) -> HashMap<ipnetwork::IpNetwork, Route> {
        let mut best_routes = HashMap::new();

        // Build adjacency list from active links
        let mut adjacency: HashMap<IpAddr, Vec<(IpAddr, u32)>> = HashMap::new();

        for link in &self.topology.links {
            if link.active {
                let weight = self.calculate_link_weight(link);
                adjacency.entry(link.from).or_insert_with(Vec::new).push((link.to, weight));
                adjacency.entry(link.to).or_insert_with(Vec::new).push((link.from, weight));
            }
        }

        // Dijkstra's algorithm for each known network
        for (network, _) in &self.topology.routes {
            if let Some(route) = self.dijkstra(&adjacency, *network) {
                best_routes.insert(*network, route);
            }
        }

        self.topology.routes = best_routes.clone();
        best_routes
    }

    fn calculate_link_weight(&self, link: &Link) -> u32 {
        // Combine hop count and bandwidth in weight calculation
        let bandwidth_factor = if link.capacity > 0 {
            1000000 / link.capacity as u32 // Higher bandwidth = lower weight
        } else {
            1000000
        };

        1 + bandwidth_factor // Base hop count + bandwidth factor
    }

    fn dijkstra(&self, adjacency: &HashMap<IpAddr, Vec<(IpAddr, u32)>>, destination: ipnetwork::IpNetwork) -> Option<Route> {
        let mut distances: HashMap<IpAddr, u32> = HashMap::new();
        let mut previous: HashMap<IpAddr, IpAddr> = HashMap::new();
        let mut heap = BinaryHeap::new();

        distances.insert(self.local_router, 0);
        heap.push(Reverse((0, self.local_router)));

        while let Some(Reverse((current_distance, current_router))) = heap.pop() {
            if current_distance > distances.get(&current_router).copied().unwrap_or(u32::MAX) {
                continue;
            }

            if let Some(neighbors) = adjacency.get(&current_router) {
                for &(neighbor, weight) in neighbors {
                    let new_distance = current_distance + weight;

                    if new_distance < distances.get(&neighbor).copied().unwrap_or(u32::MAX) {
                        distances.insert(neighbor, new_distance);
                        previous.insert(neighbor, current_router);
                        heap.push(Reverse((new_distance, neighbor)));
                    }
                }
            }
        }

        // Find the best route to the destination network
        let mut best_route: Option<Route> = None;
        let mut best_distance = u32::MAX;

        for (router_ip, distance) in distances {
            if router_ip != self.local_router && distance < best_distance {
                // Build path
                let mut path = vec![router_ip];
                let mut current = router_ip;

                while let Some(&prev) = previous.get(&current) {
                    if prev == self.local_router {
                        break;
                    }
                    path.push(prev);
                    current = prev;
                }
                path.reverse();

                let next_hop = if path.is_empty() {
                    router_ip
                } else {
                    path[0]
                };

                best_route = Some(Route {
                    destination,
                    next_hop,
                    metric: distance,
                    hop_count: path.len() as u32 + 1,
                    path,
                });
                best_distance = distance;
            }
        }

        best_route
    }

    pub fn get_neighbors(&self) -> Vec<Router> {
        self.topology.routers.values().cloned().collect()
    }

    pub fn update_link_status(&mut self, from: IpAddr, to: IpAddr, active: bool) {
        for link in &mut self.topology.links {
            if (link.from == from && link.to == to) || (link.from == to && link.to == from) {
                link.active = active;
            }
        }
    }
}
