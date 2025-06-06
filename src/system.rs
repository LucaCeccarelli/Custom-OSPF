use crate::types::Route;
use std::process::Command;
use std::collections::HashMap;
use ipnetwork::IpNetwork;

pub struct SystemIntegration;

impl SystemIntegration {
    pub fn update_routing_table(routes: &HashMap<IpNetwork, Route>) -> Result<(), Box<dyn std::error::Error>> {
        // Clear existing routes (be careful with this in production)
        // Self::clear_custom_routes()?;

        for (network, route) in routes {
            Self::add_route(network, route)?;
        }

        Ok(())
    }

    fn add_route(network: &IpNetwork, route: &Route) -> Result<(), Box<dyn std::error::Error>> {
        let output = Command::new("ip")
            .arg("route")
            .arg("add")
            .arg(network.to_string())
            .arg("via")
            .arg(route.next_hop.to_string())
            .arg("metric")
            .arg(route.metric.to_string())
            .output()?;

        if !output.status.success() {
            eprintln!("Failed to add route: {}", String::from_utf8_lossy(&output.stderr));
        }

        Ok(())
    }

    fn clear_custom_routes() -> Result<(), Box<dyn std::error::Error>> {
        // Implementation to clear routes added by our protocol
        // This should be implemented carefully to avoid removing system routes
        Ok(())
    }

    pub fn get_interface_list() -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let output = Command::new("ip")
            .arg("link")
            .arg("show")
            .output()?;

        let output_str = String::from_utf8(output.stdout)?;
        let interfaces: Vec<String> = output_str
            .lines()
            .filter_map(|line| {
                if line.contains(": ") && !line.starts_with(' ') {
                    let parts: Vec<&str> = line.split(": ").collect();
                    if parts.len() > 1 {
                        Some(parts[1].split('@').next().unwrap().to_string())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        Ok(interfaces)
    }
}
