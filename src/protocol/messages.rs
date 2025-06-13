use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use crate::{RouterId, NetworkId};
use super::Neighbor;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProtocolMessage {
    Hello(HelloMessage),
    LSA(LSAMessage), // Link State Advertisement
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloMessage {
    pub router_id: RouterId,
    pub router_name: String,
    pub interface_ip: IpAddr,
    pub network: NetworkId,
    pub bandwidth: u64,
    pub sequence: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LSAMessage {
    pub router_id: RouterId,
    pub sequence: u32,
    pub neighbors: Vec<Neighbor>,
    pub networks: Vec<NetworkId>,
}
