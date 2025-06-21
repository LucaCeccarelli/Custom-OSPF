use crate::network::{InterfaceInfo, Network};
use crate::routing_table::{RouteEntry, RouteSource, RoutingTable};
use ipnetwork::Ipv4Network;
use std::collections::HashMap;
use std::net::Ipv4Addr;

#[derive(Debug, Clone)]
pub struct RouterInfo {
    pub router_id: String,
    pub interfaces: HashMap<String, InterfaceInfo>,
}

#[derive(Debug)]
pub struct Router {
    pub id: String,
    pub routing_table: RoutingTable,
    pub network: Network,
    pub interfaces: HashMap<String, InterfaceInfo>,
}

impl Router {
    pub fn new(id: String, mut network: Network, interface_names: Vec<String>) -> Result<Self, Box<dyn std::error::Error>> {
        network.discover_interfaces(&interface_names)?;
        let interfaces = network.get_interfaces().clone();

        let mut router = Router {
            id,
            routing_table: RoutingTable::new(),
            network,
            interfaces,
        };

        // Ajouter les routes directement connectées
        router.add_connected_routes();

        Ok(router)
    }

    fn add_connected_routes(&mut self) {
        for (_, interface) in &self.interfaces {
            let route = RouteEntry {
                destination: interface.network,
                next_hop: Ipv4Addr::new(0, 0, 0, 0), // Route directe
                interface: interface.name.clone(),
                metric: 0,
                source: RouteSource::Connected,
            };
            self.routing_table.add_route(route);
        }
    }

    pub fn update_routing_table(&mut self, routes: Vec<RouteEntry>) -> Result<(), Box<dyn std::error::Error>> {
        // Supprimer les anciennes routes du protocole
        self.routing_table.clear_protocol_routes();

        // Ajouter les routes connectées
        self.add_connected_routes();

        // Ajouter les nouvelles routes du protocole
        for route in routes {
            if route.source == RouteSource::Protocol {
                self.routing_table.add_route(route);
            }
        }

        // Appliquer les changements au système
        self.apply_routes_to_system()?;

        Ok(())
    }

    fn apply_routes_to_system(&self) -> Result<(), Box<dyn std::error::Error>> {
        for route in self.routing_table.get_routes() {
            if route.source == RouteSource::Protocol {
                self.network.add_route(
                    route.destination,
                    route.next_hop,
                    &route.interface,
                    route.metric,
                )?;
            }
        }
        Ok(())
    }

    pub fn get_router_info(&self) -> RouterInfo {
        RouterInfo {
            router_id: self.id.clone(),
            interfaces: self.interfaces.clone(),
        }
    }
}
