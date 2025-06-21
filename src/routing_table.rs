use ipnetwork::Ipv4Network;
use log::{info, warn};
use std::net::Ipv4Addr;
use std::process::Command;

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
}

impl RoutingTable {
    pub fn new() -> Self {
        Self {
            routes: Vec::new(),
        }
    }

    pub fn add_route(&mut self, route: RouteEntry) -> Result<(), Box<dyn std::error::Error>> {
        // Vérifier si une route identique existe déjà
        if let Some(existing_index) = self.find_route_index(&route.destination) {
            let existing_route = &self.routes[existing_index];

            // Si la nouvelle route a une meilleure métrique, remplacer
            if route.metric < existing_route.metric {
                info!("Updating route to {} (old metric: {}, new metric: {})", 
                      route.destination, existing_route.metric, route.metric);

                // Supprimer l'ancienne route du système
                self.delete_system_route(existing_route.destination)?;

                // Remplacer dans notre table
                self.routes[existing_index] = route.clone();

                // Ajouter la nouvelle route au système
                self.add_system_route(&route)?;
            } else {
                // Garder la route existante (meilleure métrique)
                return Ok(());
            }
        } else {
            // Nouvelle route
            info!("Adding new route to {} via {} metric {}", 
                  route.destination, 
                  if route.next_hop.is_unspecified() { "direct".to_string() } else { route.next_hop.to_string() },
                  route.metric);

            self.routes.push(route.clone());

            // Ajouter au système seulement si ce n'est pas une route directe
            if route.source != RouteSource::Direct {
                self.add_system_route(&route)?;
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

    fn add_system_route(&self, route: &RouteEntry) -> Result<(), Box<dyn std::error::Error>> {
        let mut cmd = Command::new("ip");
        cmd.args(&["route", "add", &route.destination.to_string()]);

        if !route.next_hop.is_unspecified() {
            cmd.args(&["via", &route.next_hop.to_string()]);
        }

        cmd.args(&["dev", &route.interface]);
        cmd.args(&["metric", &route.metric.to_string()]);

        let output = cmd.output()?;

        if output.status.success() {
            info!("✓ Added system route: {} via {} dev {}", 
                  route.destination, 
                  if route.next_hop.is_unspecified() { "direct".to_string() } else { route.next_hop.to_string() },
                  route.interface);
        } else {
            let error = String::from_utf8_lossy(&output.stderr);
            // Ne pas considérer "File exists" comme une erreur fatale
            if !error.contains("File exists") {
                warn!("Failed to add system route: {}: {}", route.destination, error);
            }
        }

        Ok(())
    }

    fn delete_system_route(&self, destination: Ipv4Network) -> Result<(), Box<dyn std::error::Error>> {
        let output = Command::new("ip")
            .args(&["route", "del", &destination.to_string()])
            .output()?;

        if output.status.success() {
            info!("✓ Deleted system route: {}", destination);
        } else {
            let error = String::from_utf8_lossy(&output.stderr);
            if !error.contains("No such process") {
                warn!("Failed to delete system route: {}: {}", destination, error);
            }
        }

        Ok(())
    }
}

impl Drop for RoutingTable {
    fn drop(&mut self) {
        // Nettoyer les routes du protocole au shutdown
        for route in &self.routes {
            if route.source == RouteSource::Protocol {
                let _ = self.delete_system_route(route.destination);
            }
        }
    }
}
