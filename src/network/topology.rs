use std::collections::HashMap;
use crate::{RouterId, NetworkId};
use crate::protocol::{Neighbor, LSAMessage};

#[derive(Debug, Clone)]
pub struct Topology {
    pub routers: HashMap<RouterId, RouterInfo>,
    pub links: Vec<Link>,
}

#[derive(Debug, Clone)]
pub struct RouterInfo {
    pub id: RouterId,
    pub name: String,
    pub networks: Vec<NetworkId>,
}

#[derive(Debug, Clone)]
pub struct Link {
    pub from_router: RouterId,
    pub to_router: RouterId,
    pub network: NetworkId,
    pub bandwidth: u64,
    pub available_bandwidth: u64,
    pub active: bool,
}

impl Topology {
    pub fn new() -> Self {
        Self {
            routers: HashMap::new(),
            links: Vec::new(),
        }
    }

    pub fn add_neighbor(&mut self, neighbor: Neighbor) {
        let router_info = RouterInfo {
            id: neighbor.router_id.clone(),
            name: neighbor.router_name.clone(),
            networks: vec![neighbor.network],
        };

        self.routers.insert(neighbor.router_id.clone(), router_info);

        // Update or add link information
        self.update_link_info(&neighbor);
    }

    pub fn remove_neighbor(&mut self, router_id: &str) {
        self.routers.remove(router_id);
        self.links.retain(|link| {
            link.from_router != router_id && link.to_router != router_id
        });
    }

    pub fn update_from_lsa(&mut self, lsa: &LSAMessage) {
        let router_info = RouterInfo {
            id: lsa.router_id.clone(),
            name: lsa.router_id.clone(),
            networks: lsa.networks.clone(),
        };

        self.routers.insert(lsa.router_id.clone(), router_info);

        // Update links based on LSA neighbor information
        for neighbor in &lsa.neighbors {
            self.update_link_between(&lsa.router_id, &neighbor.router_id, &neighbor);
        }
    }

    fn update_link_info(&mut self, neighbor: &Neighbor) {
        // Find existing link or create new one
        if let Some(link) = self.links.iter_mut().find(|l| {
            (l.from_router == neighbor.router_id || l.to_router == neighbor.router_id) &&
                l.network == neighbor.network
        }) {
            link.bandwidth = neighbor.bandwidth;
            link.available_bandwidth = neighbor.bandwidth;
            link.active = true;
        }
    }

    fn update_link_between(&mut self, router1: &str, router2: &str, neighbor: &Neighbor) {
        // Check if link already exists
        let link_exists = self.links.iter_mut().any(|link| {
            (link.from_router == router1 && link.to_router == router2) ||
                (link.from_router == router2 && link.to_router == router1)
        });

        if !link_exists {
            let link = Link {
                from_router: router1.to_string(),
                to_router: router2.to_string(),
                network: neighbor.network,
                bandwidth: neighbor.bandwidth,
                available_bandwidth: neighbor.bandwidth,
                active: true,
            };
            self.links.push(link);
        }
    }

    pub fn get_neighbors(&self, router_id: &str) -> Vec<(String, u32)> {
        self.links
            .iter()
            .filter_map(|link| {
                if link.from_router == router_id && link.active {
                    Some((link.to_router.clone(), self.calculate_link_cost(link)))
                } else if link.to_router == router_id && link.active {
                    Some((link.from_router.clone(), self.calculate_link_cost(link)))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn get_all_networks(&self) -> Vec<NetworkId> {
        let mut networks = Vec::new();
        for router_info in self.routers.values() {
            networks.extend(router_info.networks.iter());
        }
        networks.sort();
        networks.dedup();
        networks
    }

    fn calculate_link_cost(&self, link: &Link) -> u32 {
        // Cost based on available bandwidth (lower bandwidth = higher cost)
        // Using a reference bandwidth of 1 Gbps (1_000_000_000 bps)
        let reference_bandwidth = 1_000_000_000u64;
        if link.available_bandwidth > 0 {
            (reference_bandwidth / link.available_bandwidth).max(1) as u32
        } else {
            u32::MAX // Link is down
        }
    }
}
