use ipnetwork::Ipv4Network;
use std::collections::HashMap;
use std::net::Ipv4Addr;

#[derive(Debug, Clone)]
pub struct RouteEntry {
    pub destination: Ipv4Network,
    pub next_hop: Ipv4Addr,
    pub interface: String,
    pub metric: u32,
    pub source: RouteSource,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RouteSource {
    Connected,
    Protocol,
}

#[derive(Debug)]
pub struct RoutingTable {
    routes: HashMap<String, RouteEntry>, // Key: destination network as string
}

impl RoutingTable {
    pub fn new() -> Self {
        Self {
            routes: HashMap::new(),
        }
    }

    pub fn add_route(&mut self, entry: RouteEntry) {
        let key = entry.destination.to_string();

        // Vérifier si on a déjà une route pour cette destination
        if let Some(existing) = self.routes.get(&key) {
            // Garder la route avec la meilleure métrique
            if entry.metric < existing.metric {
                self.routes.insert(key, entry);
            }
        } else {
            self.routes.insert(key, entry);
        }
    }

    pub fn remove_route(&mut self, destination: &Ipv4Network) {
        let key = destination.to_string();
        self.routes.remove(&key);
    }

    pub fn get_routes(&self) -> Vec<&RouteEntry> {
        self.routes.values().collect()
    }

    pub fn find_route(&self, destination: &Ipv4Network) -> Option<&RouteEntry> {
        let key = destination.to_string();
        self.routes.get(&key)
    }

    pub fn clear_protocol_routes(&mut self) {
        self.routes.retain(|_, route| route.source == RouteSource::Connected);
    }
}
