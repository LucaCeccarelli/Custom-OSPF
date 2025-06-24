use ipnetwork::Ipv4Network;
use log::{info, warn, debug};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use pnet::datalink;
use pnet::ipnetwork::IpNetwork;

#[derive(Debug, Clone)]
pub struct InterfaceInfo {
    pub name: String,
    pub ip: Ipv4Addr,
    pub network: Ipv4Network,
    pub metric: u32,
}

pub struct Network {
    interfaces: HashMap<String, InterfaceInfo>,
}

impl Network {
    pub fn new() -> Self {
        Self {
            interfaces: HashMap::new(),
        }
    }

    pub fn discover_interfaces(&mut self, interface_names: &[String]) -> Result<(), Box<dyn std::error::Error>> {
        info!("Discovering interfaces: {:?}", interface_names);

        for interface_name in interface_names {
            if let Ok(interface_info) = self.get_interface_info(interface_name) {
                info!("Found interface: {} -> {} ({})", 
                      interface_info.name, interface_info.ip, interface_info.network);
                self.interfaces.insert(interface_name.clone(), interface_info);
            } else {
                warn!("Interface {} not found or has no IP", interface_name);
            }
        }

        if self.interfaces.is_empty() {
            return Err("No valid interfaces found".into());
        }

        Ok(())
    }

    fn get_interface_info(&self, interface_name: &str) -> Result<InterfaceInfo, Box<dyn std::error::Error>> {
        // Get all network interfaces
        let interfaces = datalink::interfaces();

        // Find the interface by name
        let target_interface = interfaces
            .into_iter()
            .find(|iface| iface.name == interface_name)
            .ok_or_else(|| format!("Interface {} not found", interface_name))?;

        // Check if interface is up
        if !target_interface.is_up() {
            return Err(format!("Interface {} is not up", interface_name).into());
        }

        // Find the first IPv4 address on this interface
        for ip_network in target_interface.ips {
            if let IpNetwork::V4(ipv4_network) = ip_network {
                // Skip loopback addresses
                if ipv4_network.ip().is_loopback() {
                    continue;
                }

                debug!("Found IPv4 network {} on interface {}", ipv4_network, interface_name);

                return Ok(InterfaceInfo {
                    name: interface_name.to_string(),
                    ip: ipv4_network.ip(),
                    network: ipv4_network,
                    metric: 1,
                });
            }
        }

        Err(format!("No IPv4 address found for interface {}", interface_name).into())
    }

    pub fn get_interfaces(&self) -> &HashMap<String, InterfaceInfo> {
        &self.interfaces
    }
}