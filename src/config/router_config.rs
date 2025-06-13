use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use crate::{RouterId, NetworkId};
use crate::network::NetworkInterface;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    pub router_id: RouterId,
    pub router_name: String,
    pub enabled: bool,
    pub is_default_router: bool,
    pub interfaces: HashMap<String, InterfaceConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceConfig {
    pub name: String,
    pub ip_address: IpAddr,
    pub network: NetworkId,
    pub bandwidth: u64,
    pub enabled: bool,
    pub include_in_routing: bool,
}

impl RouterConfig {
    pub fn new(router_id: RouterId, router_name: String) -> Self {
        Self {
            router_id,
            router_name,
            enabled: false,
            is_default_router: false,
            interfaces: HashMap::new(),
        }
    }

    pub fn add_interface(&mut self, config: InterfaceConfig) {
        self.interfaces.insert(config.name.clone(), config);
    }

    pub fn to_network_interfaces(&self) -> HashMap<String, NetworkInterface> {
        self.interfaces
            .iter()
            .map(|(name, config)| {
                let interface = NetworkInterface {
                    name: config.name.clone(),
                    ip_address: config.ip_address,
                    network: config.network,
                    bandwidth: config.bandwidth,
                    enabled: config.enabled,
                    include_in_routing: config.include_in_routing,
                };
                (name.clone(), interface)
            })
            .collect()
    }

    pub fn load_from_file(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: RouterConfig = serde_json::from_str(&content)?;
        Ok(config)
    }

    pub fn save_to_file(&self, path: &str) -> anyhow::Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}
