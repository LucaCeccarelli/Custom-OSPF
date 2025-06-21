use ipnetwork::Ipv4Network;
use log::{info, warn, debug};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::process::Command;

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
                info!("✓ Found interface: {} -> {} ({})", 
                      interface_info.name, interface_info.ip, interface_info.network);
                self.interfaces.insert(interface_name.clone(), interface_info);
            } else {
                warn!("✗ Interface {} not found or has no IP", interface_name);
            }
        }

        if self.interfaces.is_empty() {
            return Err("No valid interfaces found".into());
        }

        Ok(())
    }

    fn get_interface_info(&self, interface_name: &str) -> Result<InterfaceInfo, Box<dyn std::error::Error>> {
        let output = Command::new("ip")
            .args(&["addr", "show", interface_name])
            .output()?;

        if !output.status.success() {
            return Err(format!("Interface {} not found", interface_name).into());
        }

        let output_str = String::from_utf8_lossy(&output.stdout);

        // Chercher l'adresse IPv4
        for line in output_str.lines() {
            if line.contains("inet ") && !line.contains("inet6") {
                let parts: Vec<&str> = line.trim().split_whitespace().collect();
                if parts.len() >= 2 && parts[0] == "inet" {
                    let addr_with_prefix = parts[1];
                    let network: Ipv4Network = addr_with_prefix.parse()?;
                    let ip = network.ip();

                    return Ok(InterfaceInfo {
                        name: interface_name.to_string(),
                        ip,
                        network,
                        metric: 1,
                    });
                }
            }
        }

        Err(format!("No IPv4 address found for interface {}", interface_name).into())
    }

    pub fn get_interfaces(&self) -> &HashMap<String, InterfaceInfo> {
        &self.interfaces
    }

    pub fn add_route(
        &self,
        destination: Ipv4Network,
        next_hop: Ipv4Addr,
        interface: &str,
        metric: u32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        debug!("Adding system route: {} via {} dev {} metric {}", 
               destination, next_hop, interface, metric);

        let mut cmd = Command::new("ip");
        cmd.args(&["route", "add", &destination.to_string()]);

        if !next_hop.is_unspecified() {
            cmd.args(&["via", &next_hop.to_string()]);
        }

        cmd.args(&["dev", interface, "metric", &metric.to_string()]);

        let output = cmd.output()?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            if !error.contains("File exists") {
                return Err(format!("Failed to add route: {}", error).into());
            }
        }

        Ok(())
    }
}
