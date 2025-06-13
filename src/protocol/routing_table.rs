use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use crate::NetworkId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingTable {
    entries: HashMap<NetworkId, RoutingEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingEntry {
    pub destination: NetworkId,
    pub next_hop: IpAddr,
    pub interface: String,
    pub metric: u32,
    pub route_type: RouteType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RouteType {
    Internal,
    External,
    Default,
}

impl RoutingTable {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn add_route(&mut self, entry: RoutingEntry) {
        self.entries.insert(entry.destination, entry);
    }

    pub fn remove_route(&mut self, destination: &NetworkId) {
        self.entries.remove(destination);
    }

    pub fn get_route(&self, destination: &NetworkId) -> Option<&RoutingEntry> {
        self.entries.get(destination)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&NetworkId, &RoutingEntry)> {
        self.entries.iter()
    }

    pub fn find_best_route(&self, target: IpAddr) -> Option<&RoutingEntry> {
        let mut best_route = None;
        let mut longest_prefix = 0;

        for (network, entry) in &self.entries {
            if network.contains(target) && network.prefix() >= longest_prefix {
                longest_prefix = network.prefix();
                best_route = Some(entry);
            }
        }

        best_route
    }
}
