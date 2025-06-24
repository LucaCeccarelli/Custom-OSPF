use crate::network::{InterfaceInfo, Network};
use crate::routing_table::{RouteEntry, RouteSource, RoutingTable};
use log::{info, warn, debug};
use std::collections::HashMap;
use std::net::Ipv4Addr;

#[derive(Debug, Clone)]
pub struct RouterInfo {
    pub router_id: String,
    pub interfaces: HashMap<String, InterfaceInfo>,
}

pub struct Router {
    pub id: String,
    pub interfaces: HashMap<String, InterfaceInfo>,
    pub routing_table: RoutingTable,
    network: Network,
}

impl Router {
    pub async fn new(
        id: String,
        mut network: Network,
        interface_names: Vec<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        info!("Creating router {}", id);

        network.discover_interfaces(&interface_names)?;
        let interfaces = network.get_interfaces().clone();

        let mut routing_table = RoutingTable::new().await?;

        // Add direct routes
        for (name, interface_info) in &interfaces {
            let route = RouteEntry {
                destination: interface_info.network,
                next_hop: std::net::Ipv4Addr::new(0, 0, 0, 0), // Direct route
                interface: name.clone(),
                metric: 0,
                source: RouteSource::Direct,
            };

            routing_table.add_route(route).await?;
        }

        Ok(Router {
            id,
            interfaces,
            routing_table,
            network,
        })
    }

    pub fn get_router_info(&self) -> RouterInfo {
        RouterInfo {
            router_id: self.id.clone(),
            interfaces: self.interfaces.clone(),
        }
    }

    pub async fn update_routing_table(&mut self, new_routes: Vec<RouteEntry>) -> Result<(), Box<dyn std::error::Error>> {
        for route in new_routes {
            // Enhanced route comparison - use replace_route for equal metrics to enable failover
            if !self.routing_table.has_better_route(&route) {
                // For equal metrics, force replacement to enable failover
                if let Some(existing) = self.routing_table.find_route(&route.destination) {
                    if existing.metric == route.metric && existing.next_hop != route.next_hop {
                        info!("Replacing route to {} (failover from {} to {})",
                              route.destination, existing.next_hop, route.next_hop);
                        self.routing_table.replace_route(route).await?;
                    } else {
                        self.routing_table.add_route(route).await?;
                    }
                } else {
                    self.routing_table.add_route(route).await?;
                }
            } else {
                debug!("Skipping route to {} - existing route is better", route.destination);
            }
        }

        Ok(())
    }

    // New method to handle dead neighbors
    pub async fn handle_dead_neighbor(&mut self, neighbor_ip: Ipv4Addr) -> Result<(), Box<dyn std::error::Error>> {
        info!("Handling dead neighbor: {}", neighbor_ip);

        // Remove all routes that go through this neighbor
        let removed_routes = self.routing_table.remove_routes_via_nexthop(neighbor_ip).await?;

        if !removed_routes.is_empty() {
            info!("Removed {} routes due to dead neighbor {}", removed_routes.len(), neighbor_ip);

            // Log what routes were affected
            for route in &removed_routes {
                info!("Removed route to {} (was via {})", route.destination, neighbor_ip);
            }
        }

        Ok(())
    }

    // New method to get routing statistics
    pub fn get_routing_stats(&self) -> String {
        let (direct, protocol, static_routes) = self.routing_table.get_stats();
        let total = direct + protocol + static_routes;

        format!("Routes: {} total ({} direct, {} protocol, {} static)",
                total, direct, protocol, static_routes)
    }

    // Enhanced method to find best route to destination
    pub fn find_best_route(&self, destination: Ipv4Addr) -> Option<&RouteEntry> {
        self.routing_table.lookup_route(destination)
    }

    // Method to check if we can reach a destination
    pub fn can_reach(&self, destination: Ipv4Addr) -> bool {
        self.routing_table.lookup_route(destination).is_some()
    }

    // Method to get next hop for a destination
    pub fn get_next_hop(&self, destination: Ipv4Addr) -> Option<Ipv4Addr> {
        self.routing_table.lookup_route(destination)
            .map(|route| {
                if route.next_hop.is_unspecified() {
                    // Direct route - destination is directly reachable
                    destination
                } else {
                    route.next_hop
                }
            })
    }

    // Method to validate neighbor connectivity
    pub fn is_neighbor_reachable(&self, neighbor_ip: Ipv4Addr) -> bool {
        // Check if the neighbor is in any of our directly connected networks
        for (_, interface) in &self.interfaces {
            if interface.network.contains(neighbor_ip) {
                return true;
            }
        }
        false
    }

    // Method to get interface for reaching a specific IP
    pub fn get_interface_for_destination(&self, destination: Ipv4Addr) -> Option<&InterfaceInfo> {
        if let Some(route) = self.routing_table.lookup_route(destination) {
            self.interfaces.get(&route.interface)
        } else {
            None
        }
    }

    // Method to refresh direct routes (useful after interface changes)
    pub async fn refresh_direct_routes(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        info!("Refreshing direct routes for router {}", self.id);

        // Re-discover interfaces
        let interface_names: Vec<String> = self.interfaces.keys().cloned().collect();
        self.network.discover_interfaces(&interface_names)?;
        let updated_interfaces = self.network.get_interfaces().clone();

        // Update our interface information
        self.interfaces = updated_interfaces;

        // Add any new direct routes
        for (name, interface_info) in &self.interfaces {
            let route = RouteEntry {
                destination: interface_info.network,
                next_hop: std::net::Ipv4Addr::new(0, 0, 0, 0),
                interface: name.clone(),
                metric: 0,
                source: RouteSource::Direct,
            };

            // This will only add if the route doesn't already exist
            if !self.routing_table.has_better_route(&route) {
                info!("Adding new direct route: {}", route.destination);
                self.routing_table.add_route(route).await?;
            }
        }

        Ok(())
    }
}