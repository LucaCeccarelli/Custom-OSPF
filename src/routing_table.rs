use ipnetwork::Ipv4Network;
use log::{info, warn, debug};
use std::net::{Ipv4Addr, IpAddr};
use pnet::datalink;
use pnet::ipnetwork::IpNetwork;

#[derive(Debug, Clone, PartialEq)]
pub enum RouteSource {
    Direct,      // Route directement connectée
    Protocol,    // Route apprise par protocole
    Static,      // Route statique
}

#[derive(Debug, Clone)]
pub struct RouteEntry {
    pub destination: Ipv4Network,
    pub next_hop: Ipv4Addr,
    pub interface: String,
    pub metric: u32,
    pub source: RouteSource,
}

pub struct RoutingTable {
    routes: Vec<RouteEntry>,
    route_handle: net_route::Handle,
    local_networks: Vec<Ipv4Network>,
}

impl RoutingTable {
    pub async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let route_handle = net_route::Handle::new()?;

        // Discover local networks at startup
        let local_networks = Self::discover_local_networks();
        debug!("Discovered local networks: {:?}", local_networks);

        Ok(Self {
            routes: Vec::new(),
            route_handle,
            local_networks,
        })
    }

    fn discover_local_networks() -> Vec<Ipv4Network> {
        let mut networks = Vec::new();
        let interfaces = datalink::interfaces();

        for iface in interfaces {
            if iface.is_up() && !iface.is_loopback() {
                for ip_network in iface.ips {
                    if let IpNetwork::V4(ipv4_network) = ip_network {
                        networks.push(ipv4_network);
                        debug!("Found local network: {} on interface {}", ipv4_network, iface.name);
                    }
                }
            }
        }

        networks
    }

    pub async fn add_route(&mut self, route: RouteEntry) -> Result<(), Box<dyn std::error::Error>> {
        // Validate route before adding
        if !self.is_valid_route(&route) {
            debug!("Route validation failed for {}", route.destination);
            return Ok(());
        }

        // Check if route already exists
        if let Some(existing_index) = self.find_route_index(&route.destination) {
            let existing_route = &self.routes[existing_index];
            if route.metric < existing_route.metric {
                info!("Updating route to {} (old metric: {}, new metric: {})",
                      route.destination, existing_route.metric, route.metric);

                // Delete old route
                self.delete_system_route(existing_route).await?;

                // Update in our table
                self.routes[existing_index] = route.clone();

                // Add new route
                if route.source != RouteSource::Direct {
                    self.add_system_route(&route).await?;
                }
            } else {
                debug!("Keeping existing route with better metric");
                return Ok(());
            }
        } else {
            info!("Adding new route to {} via {} metric {}",
                  route.destination,
                  if route.next_hop.is_unspecified() { "direct".to_string() } else { route.next_hop.to_string() },
                  route.metric);

            self.routes.push(route.clone());

            if route.source != RouteSource::Direct {
                self.add_system_route(&route).await?;
            }
        }

        Ok(())
    }

    pub fn has_better_route(&self, route: &RouteEntry) -> bool {
        if let Some(existing_route) = self.find_route(&route.destination) {
            existing_route.metric <= route.metric
        } else {
            false
        }
    }

    pub fn get_routes(&self) -> &[RouteEntry] {
        &self.routes
    }

    fn find_route(&self, destination: &Ipv4Network) -> Option<&RouteEntry> {
        self.routes.iter().find(|route| route.destination == *destination)
    }

    fn find_route_index(&self, destination: &Ipv4Network) -> Option<usize> {
        self.routes.iter().position(|route| route.destination == *destination)
    }

    fn is_valid_route(&self, route: &RouteEntry) -> bool {
        // Skip validation for direct routes
        if route.source == RouteSource::Direct {
            return true;
        }

        // Check if gateway is valid
        if route.next_hop.is_loopback() {
            debug!("Invalid gateway (loopback): {}", route.next_hop);
            return false;
        }

        // Allow unspecified for direct routes
        if route.next_hop.is_unspecified() && route.source != RouteSource::Direct {
            debug!("Invalid gateway (unspecified for non-direct route): {}", route.next_hop);
            return false;
        }

        // Check if we're not adding a route to our own local network
        for local_net in &self.local_networks {
            if route.destination.network() == local_net.network() &&
                route.destination.prefix() == local_net.prefix() {
                debug!("Skipping route to local network {}", route.destination);
                return false;
            }
        }

        // Check if gateway is reachable (in one of our local networks)
        let mut gateway_is_local = false;
        for local_net in &self.local_networks {
            if local_net.contains(route.next_hop) {
                debug!("Gateway {} found in local network {}", route.next_hop, local_net);
                gateway_is_local = true;
                break;
            }
        }

        if !gateway_is_local {
            debug!("Gateway {} is not in any local network", route.next_hop);
            return false;
        }

        true
    }

    async fn add_system_route(&self, route: &RouteEntry) -> Result<(), Box<dyn std::error::Error>> {
        info!("Adding route via net-route: {} via {} metric {}",
              route.destination, route.next_hop, route.metric);

        // Create the route with proper network address (not host address)
        let destination_network = route.destination.network();
        let prefix_len = route.destination.prefix();

        debug!("Creating route: network={}, prefix={}, gateway={}",
               destination_network, prefix_len, route.next_hop);

        let net_route = net_route::Route::new(
            IpAddr::V4(destination_network),
            prefix_len
        ).with_gateway(IpAddr::V4(route.next_hop));

        match self.route_handle.add(&net_route).await {
            Ok(_) => {
                info!("Successfully added route: {}", route.destination);
                Ok(())
            },
            Err(e) => {
                debug!("Route add failed, trying to update: {}", e);

                // Try to delete and re-add (common for updating routes)
                let _ = self.route_handle.delete(&net_route).await;
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                match self.route_handle.add(&net_route).await {
                    Ok(_) => {
                        info!("Successfully updated route: {}", route.destination);
                        Ok(())
                    },
                    Err(e2) => {
                        warn!("Failed to add/update route {}: {}", route.destination, e2);
                        // Don't fail completely - just log the error
                        Ok(())
                    }
                }
            }
        }
    }

    async fn delete_system_route(&self, route: &RouteEntry) -> Result<(), Box<dyn std::error::Error>> {
        info!("Deleting route: {}", route.destination);

        let net_route = net_route::Route::new(
            IpAddr::V4(route.destination.network()),
            route.destination.prefix()
        ).with_gateway(IpAddr::V4(route.next_hop));

        match self.route_handle.delete(&net_route).await {
            Ok(_) => {
                info!("Successfully deleted route: {}", route.destination);
                Ok(())
            },
            Err(e) => {
                debug!("Route delete failed (may not exist): {}", e);

                // Try with ip command as fallback
                let _ = self.delete_route_with_ip_command(route).await;
                Ok(()) // Not a critical error
            }
        }
    }

    async fn delete_route_with_ip_command(&self, route: &RouteEntry) -> Result<(), Box<dyn std::error::Error>> {
        use tokio::process::Command;

        let mut cmd = Command::new("ip");
        cmd.args(&["route", "del", &route.destination.to_string()]);

        if !route.next_hop.is_unspecified() {
            cmd.args(&["via", &route.next_hop.to_string()]);
        }

        let output = cmd.output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            debug!("ip route del failed: {}", stderr);
        }

        Ok(())
    }
}

impl Drop for RoutingTable {
    fn drop(&mut self) {
        // Clean up protocol routes on shutdown
        for route in &self.routes {
            if route.source == RouteSource::Protocol {
                let net_route = net_route::Route::new(
                    IpAddr::V4(route.destination.network()),
                    route.destination.prefix()
                ).with_gateway(IpAddr::V4(route.next_hop));

                // Use blocking version in Drop
                if let Err(e) = futures::executor::block_on(self.route_handle.delete(&net_route)) {
                    warn!("Failed to cleanup route {}: {}", route.destination, e);
                }
            }
        }
    }
}