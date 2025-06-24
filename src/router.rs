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
}