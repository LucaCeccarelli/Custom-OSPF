use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::collections::HashMap;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProtocolMessage {
    Hello(HelloMessage),
    LinkStateAdvertisement(LSAMessage),
    LinkStateRequest(LSRMessage),
    LinkStateUpdate(LSUMessage),
    LinkStateAcknowledgment(LSAckMessage),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloMessage {
    pub router_id: String,
    pub router_name: String,
    pub area_id: u32,
    pub hello_interval: u32,
    pub dead_interval: u32,
    pub neighbors: Vec<String>,
    pub designated_router: Option<String>,
    pub backup_designated_router: Option<String>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LSAMessage {
    pub lsa_type: LSAType,
    pub link_state_id: String,
    pub advertising_router: String,
    pub sequence_number: u32,
    pub checksum: u16,
    pub age: u16,
    pub data: LSAData,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LSAType {
    RouterLSA,
    NetworkLSA,
    SummaryLSA,
    ExternalLSA,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LSAData {
    Router(RouterLSAData),
    Network(NetworkLSAData),
    Summary(SummaryLSAData),
    External(ExternalLSAData),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterLSAData {
    pub flags: u8,
    pub links: Vec<LinkData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkData {
    pub link_id: String,
    pub link_data: IpAddr,
    pub link_type: u8,
    pub metric: u32,
    pub bandwidth: u64,
    pub available_bandwidth: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkLSAData {
    pub network_mask: IpAddr,
    pub attached_routers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryLSAData {
    pub network_mask: IpAddr,
    pub metric: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalLSAData {
    pub network_mask: IpAddr,
    pub metric: u32,
    pub external_metric_type: u8,
    pub forwarding_address: Option<IpAddr>,
    pub external_route_tag: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LSRMessage {
    pub requests: Vec<LSAHeader>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LSAHeader {
    pub lsa_type: LSAType,
    pub link_state_id: String,
    pub advertising_router: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LSUMessage {
    pub lsas: Vec<LSAMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LSAckMessage {
    pub lsa_headers: Vec<LSAHeader>,
}

impl ProtocolMessage {
    pub fn serialize(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    pub fn deserialize(data: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(data)
    }
}

impl HelloMessage {
    pub fn new(
        router_id: String,
        router_name: String,
        hello_interval: u32,
        dead_interval: u32,
        neighbors: Vec<String>,
    ) -> Self {
        Self {
            router_id,
            router_name,
            area_id: 0,
            hello_interval,
            dead_interval,
            neighbors,
            designated_router: None,
            backup_designated_router: None,
            timestamp: Utc::now(),
        }
    }
}

impl LSAMessage {
    pub fn new_router_lsa(
        router_id: String,
        sequence_number: u32,
        links: Vec<LinkData>,
    ) -> Self {
        Self {
            lsa_type: LSAType::RouterLSA,
            link_state_id: router_id.clone(),
            advertising_router: router_id,
            sequence_number,
            checksum: 0, // Calculate checksum
            age: 0,
            data: LSAData::Router(RouterLSAData {
                flags: 0,
                links,
            }),
            timestamp: Utc::now(),
        }
    }
}