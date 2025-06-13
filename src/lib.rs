pub mod protocol;
pub mod network;
pub mod algorithms;
pub mod config;

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use tokio::sync::RwLock;

pub type RouterId = String;
pub type NetworkId = ipnetwork::IpNetwork;

#[derive(Debug, Clone)]
pub struct RouterState {
    pub id: RouterId,
    pub enabled: bool,
    pub is_default_router: bool,
    pub interfaces: HashMap<String, network::NetworkInterface>,
    pub neighbors: HashMap<RouterId, protocol::Neighbor>,
    pub routing_table: protocol::RoutingTable,
    pub topology: network::Topology,
}

impl RouterState {
    pub fn new(id: RouterId) -> Self {
        Self {
            id,
            enabled: false,
            is_default_router: false,
            interfaces: HashMap::new(),
            neighbors: HashMap::new(),
            routing_table: protocol::RoutingTable::new(),
            topology: network::Topology::new(),
        }
    }
}

pub type SharedRouterState = Arc<RwLock<RouterState>>;
