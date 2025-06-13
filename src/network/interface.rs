use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use crate::NetworkId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkInterface {
    pub name: String,
    pub ip_address: IpAddr,
    pub network: NetworkId,
    pub bandwidth: u64, // bits per second
    pub enabled: bool,
    pub include_in_routing: bool,
}

impl NetworkInterface {
    pub fn new(name: String, ip_address: IpAddr, network: NetworkId, bandwidth: u64) -> Self {
        Self {
            name,
            ip_address,
            network,
            bandwidth,
            enabled: true,
            include_in_routing: true,
        }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn disable(&mut self) {
        self.enabled = false;
    }

    pub fn set_routing_enabled(&mut self, enabled: bool) {
        self.include_in_routing = enabled;
    }

    pub fn available_bandwidth(&self) -> u64 {
        // In a real implementation, this would check actual utilization
        // For now, return full bandwidth if interface is up
        if self.enabled {
            self.bandwidth
        } else {
            0
        }
    }
}
