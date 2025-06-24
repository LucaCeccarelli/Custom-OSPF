use crate::router::RouterInfo;
use crate::routing_table::RouteEntry;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Instant, SystemTime};
use tokio::sync::oneshot;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RoutingUpdate {
    pub router_id: String,
    pub sequence: u64,
    pub routes: Vec<RouteInfo>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RouteInfo {
    pub destination: String,
    pub metric: u32,
    pub next_hop: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HelloMessage {
    pub router_id: String,
    pub interfaces: HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NeighborRequest {
    pub requesting_router_id: String,
    pub request_id: String,
    pub target_router_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NeighborResponse {
    pub responding_router_id: String,
    pub request_id: String,
    pub neighbors: Vec<NeighborResponseInfo>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NeighborResponseInfo {
    pub router_id: String,
    pub ip_address: String,
    pub last_seen: String,
    pub is_alive: bool,
    pub interfaces: Vec<String>,
}

#[derive(Debug)]
pub struct PendingNeighborRequest {
    pub responder: oneshot::Sender<Vec<crate::control_server::NeighborInfo>>,
    pub timestamp: SystemTime,
}

#[derive(Debug, Clone)]
pub struct NeighborInfo {
    pub router_info: RouterInfo,
    pub last_seen: Instant,
    pub socket_addr: SocketAddr,
    pub is_alive: bool,
}

#[derive(Debug, Clone)]
pub struct RouteState {
    pub route: RouteEntry,
    pub last_advertised: Instant,
    pub advertising_neighbor: String,
}