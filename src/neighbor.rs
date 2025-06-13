use std::collections::HashMap;
use std::net::IpAddr;
use chrono::{DateTime, Utc, Duration};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Neighbor {
    pub router_id: String,
    pub router_name: String,
    pub ip_address: IpAddr,
    pub state: NeighborState,
    pub last_hello: DateTime<Utc>,
    pub dead_interval: u32,
    pub hello_interval: u32,
    pub interface: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum NeighborState {
    Down,
    Init,
    TwoWay,
    ExStart,
    Exchange,
    Loading,
    Full,
}

#[derive(Debug)]
pub struct NeighborManager {
    neighbors: HashMap<String, Neighbor>,
    dead_interval: u32,
}

impl NeighborManager {
    pub fn new(dead_interval: u32) -> Self {
        Self {
            neighbors: HashMap::new(),
            dead_interval,
        }
    }

    pub fn add_neighbor(&mut self, neighbor: Neighbor) {
        self.neighbors.insert(neighbor.router_id.clone(), neighbor);
    }

    pub fn update_neighbor_hello(&mut self, router_id: &str) -> bool {
        if let Some(neighbor) = self.neighbors.get_mut(router_id) {
            neighbor.last_hello = Utc::now();
            neighbor.state = NeighborState::TwoWay;
            true
        } else {
            false
        }
    }

    pub fn get_neighbor(&self, router_id: &str) -> Option<&Neighbor> {
        self.neighbors.get(router_id)
    }

    pub fn get_neighbors(&self) -> &HashMap<String, Neighbor> {
        &self.neighbors
    }

    pub fn get_active_neighbors(&self) -> Vec<&Neighbor> {
        self.neighbors
            .values()
            .filter(|n| n.state != NeighborState::Down)
            .collect()
    }

    pub fn remove_neighbor(&mut self, router_id: &str) -> Option<Neighbor> {
        self.neighbors.remove(router_id)
    }

    pub fn check_dead_neighbors(&mut self) -> Vec<String> {
        let now = Utc::now();
        let mut dead_neighbors = Vec::new();

        for (router_id, neighbor) in &mut self.neighbors {
            let time_since_hello = now.signed_duration_since(neighbor.last_hello);
            if time_since_hello > Duration::seconds(neighbor.dead_interval as i64) {
                neighbor.state = NeighborState::Down;
                dead_neighbors.push(router_id.clone());
            }
        }

        // Remove dead neighbors
        for router_id in &dead_neighbors {
            self.neighbors.remove(router_id);
        }

        dead_neighbors
    }

    pub fn get_neighbor_count(&self) -> usize {
        self.neighbors.len()
    }

    pub fn is_neighbor_active(&self, router_id: &str) -> bool {
        self.neighbors
            .get(router_id)
            .map(|n| n.state != NeighborState::Down)
            .unwrap_or(false)
    }

    pub fn get_neighbors_for_interface(&self, interface: &str) -> Vec<&Neighbor> {
        self.neighbors
            .values()
            .filter(|n| n.interface == interface)
            .collect()
    }
}

impl Neighbor {
    pub fn new(
        router_id: String,
        router_name: String,
        ip_address: IpAddr,
        interface: String,
        hello_interval: u32,
        dead_interval: u32,
    ) -> Self {
        Self {
            router_id,
            router_name,
            ip_address,
            state: NeighborState::Init,
            last_hello: Utc::now(),
            dead_interval,
            hello_interval,
            interface,
        }
    }

    pub fn is_active(&self) -> bool {
        self.state != NeighborState::Down
    }

    pub fn time_since_last_hello(&self) -> Duration {
        Utc::now().signed_duration_since(self.last_hello)
    }
}