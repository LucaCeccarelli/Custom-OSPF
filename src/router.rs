use crate::network::{InterfaceInfo, Network};
use crate::routing_table::{RouteEntry, RouteSource, RoutingTable};
use log::info;
use std::collections::HashMap;

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
    // Make this async since RoutingTable::new() is async
    pub async fn new(
        id: String,
        mut network: Network,
        interface_names: Vec<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        info!("Creating router {}", id);

        network.discover_interfaces(&interface_names)?;
        let interfaces = network.get_interfaces().clone();

        // Await the async RoutingTable::new()
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

            // Await the async add_route
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

    // Make this async too since add_route is async
    pub async fn update_routing_table(&mut self, new_routes: Vec<RouteEntry>) -> Result<(), Box<dyn std::error::Error>> {
        for route in new_routes {
            // Check if a better route already exists
            if !self.routing_table.has_better_route(&route) {
                // Add the route (await since it's async)
                self.routing_table.add_route(route).await?;
            }
        }

        Ok(())
    }
}