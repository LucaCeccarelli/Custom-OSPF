use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Router {
    pub id: IpAddr,
    pub hostname: String,
    pub last_seen: SystemTime,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub from: IpAddr,
    pub to: IpAddr,
    pub capacity: u64,     // Nominal bandwidth in Mbps
    pub available: u64,    // Available bandwidth in Mbps
    pub active: bool,
    pub interface: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub destination: ipnetwork::IpNetwork,
    pub next_hop: IpAddr,
    pub metric: u32,
    pub hop_count: u32,
    pub path: Vec<IpAddr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingMessage {
    pub sender: IpAddr,
    pub sequence: u32,
    pub timestamp: SystemTime,
    pub routes: Vec<Route>,
    pub neighbors: Vec<Router>,
}

#[derive(Debug, Clone)]
pub struct NetworkTopology {
    pub routers: HashMap<IpAddr, Router>,
    pub links: Vec<Link>,
    pub routes: HashMap<ipnetwork::IpNetwork, Route>,
}
