use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::collections::HashMap;
use std::fs;
use anyhow::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    pub interfaces: Vec<InterfaceConfig>,
    pub hello_interval: u32,
    pub dead_interval: u32,
    pub lsa_refresh_interval: u32,
    pub max_age: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceConfig {
    pub name: String,
    pub ip_address: IpAddr,
    pub network: String,
    pub enabled: bool,
    pub cost: u32,
    pub bandwidth: u64, // in Mbps
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            interfaces: vec![],
            hello_interval: 10,      // 10 seconds
            dead_interval: 40,       // 40 seconds  
            lsa_refresh_interval: 1800, // 30 minutes
            max_age: 3600,          // 1 hour
        }
    }
}

impl RouterConfig {
    pub fn load(path: &str) -> Result<Self> {
        let content = fs::read_to_string(path)?;
        let config: RouterConfig = serde_json::from_str(&content)?;
        Ok(config)
    }

    pub fn save(&self, path: &str) -> Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        fs::write(path, content)?;
        Ok(())
    }

    pub fn get_enabled_interfaces(&self) -> Vec<&InterfaceConfig> {
        self.interfaces.iter().filter(|i| i.enabled).collect()
    }

    pub fn get_interface_by_name(&self, name: &str) -> Option<&InterfaceConfig> {
        self.interfaces.iter().find(|i| i.name == name)
    }
}