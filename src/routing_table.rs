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
}

impl RoutingTable {
    pub async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let route_handle = net_route::Handle::new()?;

        Ok(Self {
            routes: Vec::new(),
            route_handle,
        })
    }

    pub async fn add_route(&mut self, route: RouteEntry) -> Result<(), Box<dyn std::error::Error>> {
        // Validate route before adding
        if !self.is_valid_route(&route).await? {
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

    async fn is_valid_route(&self, route: &RouteEntry) -> Result<bool, Box<dyn std::error::Error>> {
        // Skip validation for direct routes
        if route.source == RouteSource::Direct {
            return Ok(true);
        }

        // Check if gateway is valid
        if route.next_hop.is_loopback() || route.next_hop.is_unspecified() {
            debug!("Invalid gateway: {}", route.next_hop);
            return Ok(false);
        }

        // Check if gateway is reachable (in local network)
        let interfaces = datalink::interfaces();
        let mut gateway_is_local = false;

        for iface in interfaces {
            for ip_network in iface.ips {
                if let IpNetwork::V4(ipv4_network) = ip_network {
                    if ipv4_network.contains(route.next_hop) {
                        debug!("Gateway {} found in local network {}", route.next_hop, ipv4_network);
                        gateway_is_local = true;
                        break;
                    }
                }
            }
            if gateway_is_local { break; }
        }

        if !gateway_is_local {
            debug!("Gateway {} is not in any local network", route.next_hop);
            return Ok(false);
        }

        // Check if we're not adding a route to our own local network
        let interfaces = datalink::interfaces();
        for iface in interfaces {
            for ip_network in iface.ips {
                if let IpNetwork::V4(local_net) = ip_network {
                    if route.destination.network() == local_net.network() &&
                        route.destination.prefix() == local_net.prefix() {
                        debug!("Skipping route to local network {}", route.destination);
                        return Ok(false);
                    }
                }
            }
        }

        Ok(true)
    }

    async fn add_system_route(&self, route: &RouteEntry) -> Result<(), Box<dyn std::error::Error>> {
        info!("🚀 Adding route via net-route: {} via {} metric {}",
              route.destination, route.next_hop, route.metric);

        let net_route = net_route::Route::new(
            IpAddr::V4(route.destination.network()),
            route.destination.prefix()
        ).with_gateway(IpAddr::V4(route.next_hop));

        match self.route_handle.add(&net_route).await {
            Ok(_) => {
                info!("✅ Successfully added route: {}", route.destination);
                Ok(())
            },
            Err(e) => {
                debug!("Route add failed, trying to update: {}", e);

                // Try to delete and re-add
                let _ = self.route_handle.delete(&net_route).await;

                match self.route_handle.add(&net_route).await {
                    Ok(_) => {
                        info!("✅ Successfully updated route: {}", route.destination);
                        Ok(())
                    },
                    Err(e2) => {
                        warn!("❌ Failed to add/update route {}: {}", route.destination, e2);
                        // Don't fail completely - just log the error
                        Ok(())
                    }
                }
            }
        }
    }

    async fn delete_system_route(&self, route: &RouteEntry) -> Result<(), Box<dyn std::error::Error>> {
        info!("🗑️ Deleting route: {}", route.destination);

        let net_route = net_route::Route::new(
            IpAddr::V4(route.destination.network()),
            route.destination.prefix()
        ).with_gateway(IpAddr::V4(route.next_hop));

        match self.route_handle.delete(&net_route).await {
            Ok(_) => {
                info!("✅ Successfully deleted route: {}", route.destination);
                Ok(())
            },
            Err(e) => {
                debug!("Route delete failed (may not exist): {}", e);
                Ok(()) // Not a critical error
            }
        }
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
