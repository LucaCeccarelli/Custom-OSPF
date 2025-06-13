use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use serde::{Deserialize, Serialize};
use ipnet::Ipv4Net;
use anyhow::Result;

#[derive(Debug, Clone)]
pub struct RoutingEntry {
    pub destination: Ipv4Net,
    pub next_hop: Option<IpAddr>,
    pub interface: String,
    pub metric: u32,
    pub route_type: RouteType,
    pub age: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RouteType {
    Connected,
    Static,
    Dynamic,
    Default,
}

#[derive(Debug)]
pub struct RoutingTable {
    entries: HashMap<String, RoutingEntry>,
    default_route: Option<RoutingEntry>,
}

impl RoutingTable {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            default_route: None,
        }
    }

    pub fn add_route(&mut self, entry: RoutingEntry) {
        let key = format!("{}/{}", entry.destination.addr(), entry.destination.prefix_len());

        // Check if this is a better route (lower metric)
        if let Some(existing) = self.entries.get(&key) {
            if entry.metric >= existing.metric {
                return; // Keep existing better route
            }
        }

        self.entries.insert(key, entry);
    }

    pub fn remove_route(&mut self, destination: &Ipv4Net) -> Option<RoutingEntry> {
        let key = format!("{}/{}", destination.addr(), destination.prefix_len());
        self.entries.remove(&key)
    }

    pub fn get_route(&self, destination: &Ipv4Net) -> Option<&RoutingEntry> {
        let key = format!("{}/{}", destination.addr(), destination.prefix_len());
        self.entries.get(&key)
    }

    pub fn get_all_routes(&self) -> &HashMap<String, RoutingEntry> {
        &self.entries
    }

    pub fn set_default_route(&mut self, entry: RoutingEntry) {
        self.default_route = Some(entry);
    }

    pub fn get_default_route(&self) -> Option<&RoutingEntry> {
        self.default_route.as_ref()
    }

    pub fn lookup(&self, destination: IpAddr) -> Option<&RoutingEntry> {
        if let IpAddr::V4(ipv4) = destination {
            // Find the most specific matching route
            let mut best_match: Option<&RoutingEntry> = None;
            let mut best_prefix_len = 0;

            for entry in self.entries.values() {
                if entry.destination.contains(&ipv4) {
                    if entry.destination.prefix_len() > best_prefix_len {
                        best_match = Some(entry);
                        best_prefix_len = entry.destination.prefix_len();
                    }
                }
            }

            // If no specific route found, use default route
            best_match.or(self.default_route.as_ref())
        } else {
            None // IPv6 not supported in this implementation
        }
    }

    pub fn clear_dynamic_routes(&mut self) {
        self.entries.retain(|_, entry| entry.route_type != RouteType::Dynamic);
    }

    pub fn get_routes_by_type(&self, route_type: RouteType) -> Vec<&RoutingEntry> {
        self.entries
            .values()
            .filter(|entry| entry.route_type == route_type)
            .collect()
    }

    pub fn update_system_routing_table(&self) -> Result<()> {
        // In a real implementation, this would update the actual system routing table
        // For now, we'll just log the changes
        tracing::info!("Updating system routing table with {} entries", self.entries.len());

        for (dest, entry) in &self.entries {
            tracing::debug!(
                "Route: {} via {} dev {} metric {}",
                dest,
                entry.next_hop.map(|ip| ip.to_string()).unwrap_or("direct".to_string()),
                entry.interface,
                entry.metric
            );
        }

        if let Some(default) = &self.default_route {
            tracing::debug!(
                "Default route: via {} dev {} metric {}",
                default.next_hop.map(|ip| ip.to_string()).unwrap_or("direct".to_string()),
                default.interface,
                default.metric
            );
        }

        Ok(())
    }

    pub fn age_routes(&mut self) {
        for entry in self.entries.values_mut() {
            entry.age += 1;
        }

        if let Some(default) = &mut self.default_route {
            default.age += 1;
        }
    }

    pub fn remove_aged_routes(&mut self, max_age: u32) {
        self.entries.retain(|_, entry| {
            entry.route_type != RouteType::Dynamic || entry.age <= max_age
        });
    }
}

impl RoutingEntry {
    pub fn new_connected(
        destination: Ipv4Net,
        interface: String,
    ) -> Self {
        Self {
            destination,
            next_hop: None,
            interface,
            metric: 1,
            route_type: RouteType::Connected,
            age: 0,
        }
    }

    pub fn new_dynamic(
        destination: Ipv4Net,
        next_hop: IpAddr,
        interface: String,
        metric: u32,
    ) -> Self {
        Self {
            destination,
            next_hop: Some(next_hop),
            interface,
            metric,
            route_type: RouteType::Dynamic,
            age: 0,
        }
    }

    pub fn new_default(
        next_hop: IpAddr,
        interface: String,
        metric: u32,
    ) -> Self {
        Self {
            destination: Ipv4Net::new(Ipv4Addr::new(0, 0, 0, 0), 0).unwrap(),
            next_hop: Some(next_hop),
            interface,
            metric,
            route_type: RouteType::Default,
            age: 0,
        }
    }
}