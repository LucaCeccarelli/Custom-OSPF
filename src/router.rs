use crate::network::{InterfaceInfo, Network};
use crate::routing_table::{RouteEntry, RouteSource, RoutingTable};
use log::{info, warn};
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
    pub fn new(
        id: String,
        mut network: Network,
        interface_names: Vec<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        info!("Creating router {}", id);

        network.discover_interfaces(&interface_names)?;
        let interfaces = network.get_interfaces().clone();

        let mut routing_table = RoutingTable::new();

        // Ajouter les routes directes
        for (name, interface_info) in &interfaces {
            let route = RouteEntry {
                destination: interface_info.network,
                next_hop: std::net::Ipv4Addr::new(0, 0, 0, 0), // Route directe
                interface: name.clone(),
                metric: 0,
                source: RouteSource::Direct,
            };

            routing_table.add_route(route)?; // Maintenant ça fonctionne car add_route retourne Result
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

    pub fn update_routing_table(&mut self, new_routes: Vec<RouteEntry>) -> Result<(), Box<dyn std::error::Error>> {
        for route in new_routes {
            // Vérifier si une meilleure route existe déjà
            if !self.routing_table.has_better_route(&route) {
                // Ajouter la route (elle gèrera elle-même l'ajout système)
                self.routing_table.add_route(route)?;
            }
        }

        Ok(())
    }
}
