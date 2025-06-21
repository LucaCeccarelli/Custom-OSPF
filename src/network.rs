use ipnetwork::Ipv4Network;
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

#[derive(Debug)]
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
        for name in interface_names {
            if let Some(info) = self.get_interface_info(name)? {
                self.interfaces.insert(name.clone(), info);
            }
        }
        Ok(())
    }

    fn get_interface_info(&self, name: &str) -> Result<Option<InterfaceInfo>, Box<dyn std::error::Error>> {
        let output = Command::new("ip")
            .args(&["addr", "show", name])
            .output()?;

        if !output.status.success() {
            return Ok(None);
        }

        let output_str = String::from_utf8(output.stdout)?;

        // Parser simple pour extraire l'IP et le masque
        for line in output_str.lines() {
            if line.trim().starts_with("inet ") && !line.contains("127.0.0.1") {
                let parts: Vec<&str> = line.trim().split_whitespace().collect();
                if parts.len() >= 2 {
                    let ip_cidr = parts[1];
                    if let Ok(network) = ip_cidr.parse::<Ipv4Network>() {
                        return Ok(Some(InterfaceInfo {
                            name: name.to_string(),
                            ip: network.ip(),
                            network,
                            metric: 1, // Métrique par défaut
                        }));
                    }
                }
            }
        }

        Ok(None)
    }

    pub fn get_interfaces(&self) -> &HashMap<String, InterfaceInfo> {
        &self.interfaces
    }

    pub fn add_route(&self, dest: Ipv4Network, gateway: Ipv4Addr, interface: &str, metric: u32) -> Result<(), Box<dyn std::error::Error>> {
        let mut cmd = Command::new("ip");
        cmd.args(&["route", "add", &dest.to_string()]);

        if gateway != Ipv4Addr::new(0, 0, 0, 0) {
            cmd.args(&["via", &gateway.to_string()]);
        }

        cmd.args(&["dev", interface, "metric", &metric.to_string()]);

        let output = cmd.output()?;
        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            log::warn!("Failed to add route: {}", error);
        }

        Ok(())
    }

    pub fn delete_route(&self, dest: Ipv4Network) -> Result<(), Box<dyn std::error::Error>> {
        let output = Command::new("ip")
            .args(&["route", "del", &dest.to_string()])
            .output()?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            log::warn!("Failed to delete route: {}", error);
        }

        Ok(())
    }
}
