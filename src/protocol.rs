use crate::network::Network;
use crate::router::{Router, RouterInfo};
use crate::routing_table::{RouteEntry, RouteSource};
use log::{info, warn, error, debug};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::time::interval;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RoutingUpdate {
    pub router_id: String,
    pub sequence: u64,
    pub routes: Vec<RouteInfo>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RouteInfo {
    pub destination: String,
    pub metric: u32,
    pub next_hop: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HelloMessage {
    pub router_id: String,
    pub interfaces: HashMap<String, String>,
}

pub struct SimpleRoutingProtocol {
    router: Arc<Mutex<Router>>,
    socket: UdpSocket,
    neighbors: Arc<Mutex<HashMap<String, (SocketAddr, RouterInfo)>>>,
    sequence: Arc<Mutex<u64>>,
    multicast_addr: SocketAddr,
    debug_mode: bool,
}

impl SimpleRoutingProtocol {
    pub async fn new(
        router_id: String,
        interface_names: HashSet<String>,
        port: u16,
        network: Network,
        debug_mode: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", port)).await?;
        socket.set_broadcast(true)?;

        let router = Router::new(
            router_id,
            network,
            interface_names.into_iter().collect(),
        )?;

        info!("Router created successfully");
        info!("Listening on port {}", port);

        Ok(Self {
            router: Arc::new(Mutex::new(router)),
            socket,
            neighbors: Arc::new(Mutex::new(HashMap::new())),
            sequence: Arc::new(Mutex::new(0)),
            multicast_addr: format!("255.255.255.255:{}", port).parse()?,
            debug_mode,
        })
    }

    pub async fn start(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        info!("=== Starting routing protocol ===");

        // Afficher l'état initial
        self.print_initial_state().await;

        // Démarrer les tâches
        let hello_task = self.start_hello_task();
        let update_task = self.start_update_task();
        let listen_task = self.start_listen_task();
        let debug_task = if self.debug_mode {
            Some(self.start_debug_task())
        } else {
            None
        };

        // Attendre toutes les tâches
        if let Some(debug_task) = debug_task {
            tokio::try_join!(hello_task, update_task, listen_task, debug_task)?;
        } else {
            tokio::try_join!(hello_task, update_task, listen_task)?;
        }

        Ok(())
    }

    async fn print_initial_state(&self) {
        let router_guard = self.router.lock().await;
        info!("=== Initial Router State ===");
        info!("Router ID: {}", router_guard.id);
        info!("Interfaces:");
        for (name, interface) in &router_guard.interfaces {
            info!("  {} -> {} ({})", name, interface.ip, interface.network);
        }
        info!("Initial routing table:");
        for route in router_guard.routing_table.get_routes() {
            // FIX: Créer une variable temporaire pour éviter l'erreur de lifetime
            let next_hop_str = if route.next_hop.is_unspecified() {
                "direct".to_string()
            } else {
                route.next_hop.to_string()
            };

            info!("  {} via {} dev {} metric {} ({:?})", 
                  route.destination, 
                  next_hop_str,
                  route.interface, 
                  route.metric, 
                  route.source);
        }
        info!("=============================");
    }

    async fn start_debug_task(&self) -> Result<(), Box<dyn std::error::Error>> {
        let router = self.router.clone();
        let neighbors = self.neighbors.clone();

        let mut interval = interval(Duration::from_secs(30));

        loop {
            interval.tick().await;

            info!("=== DEBUG STATUS ===");

            // État des voisins
            let neighbors_guard = neighbors.lock().await;
            info!("Neighbors ({}): ", neighbors_guard.len());
            for (id, (addr, _)) in neighbors_guard.iter() {
                info!("  {} at {}", id, addr);
            }
            drop(neighbors_guard);

            // Table de routage actuelle
            let router_guard = router.lock().await;
            info!("Current routing table:");
            let routes = router_guard.routing_table.get_routes();
            if routes.is_empty() {
                info!("  (no routes)");
            } else {
                for route in routes {
                    // FIX: Même correction ici
                    let next_hop_str = if route.next_hop.is_unspecified() {
                        "direct".to_string()
                    } else {
                        route.next_hop.to_string()
                    };

                    info!("  {} via {} dev {} metric {} ({:?})", 
                          route.destination, 
                          next_hop_str,
                          route.interface, 
                          route.metric, 
                          route.source);
                }
            }
            drop(router_guard);

            info!("===================");
        }
    }

    async fn start_hello_task(&self) -> Result<(), Box<dyn std::error::Error>> {
        let socket = &self.socket;
        let router = self.router.clone();
        let multicast_addr = self.multicast_addr;

        let mut interval = interval(Duration::from_secs(10));

        loop {
            interval.tick().await;

            let router_guard = router.lock().await;
            let router_info = router_guard.get_router_info();
            drop(router_guard);

            let hello = HelloMessage {
                router_id: router_info.router_id.clone(),
                interfaces: router_info.interfaces
                    .iter()
                    .map(|(name, info)| (name.clone(), info.ip.to_string()))
                    .collect(),
            };

            let message = serde_json::to_string(&hello)?;
            let hello_packet = format!("HELLO:{}", message);

            debug!("Sending HELLO: {}", message);

            if let Err(e) = socket.send_to(hello_packet.as_bytes(), multicast_addr).await {
                warn!("Failed to send hello: {}", e);
            } else {
                info!("✓ Sent HELLO from {}", router_info.router_id);
            }
        }
    }

    async fn start_update_task(&self) -> Result<(), Box<dyn std::error::Error>> {
        let socket = &self.socket;
        let router = self.router.clone();
        let sequence = self.sequence.clone();
        let multicast_addr = self.multicast_addr;

        let mut interval = interval(Duration::from_secs(20));

        loop {
            interval.tick().await;

            let mut seq_guard = sequence.lock().await;
            *seq_guard += 1;
            let current_seq = *seq_guard;
            drop(seq_guard);

            let router_guard = router.lock().await;
            let routes: Vec<RouteInfo> = router_guard
                .routing_table
                .get_routes()
                .iter()
                .map(|route| RouteInfo {
                    destination: route.destination.to_string(),
                    metric: route.metric,
                    next_hop: route.next_hop.to_string(),
                })
                .collect();
            let router_id = router_guard.id.clone();
            drop(router_guard);

            let update = RoutingUpdate {
                router_id: router_id.clone(),
                sequence: current_seq,
                routes,
            };

            let message = serde_json::to_string(&update)?;
            let update_packet = format!("UPDATE:{}", message);

            debug!("Sending UPDATE: {}", message);

            if let Err(e) = socket.send_to(update_packet.as_bytes(), multicast_addr).await {
                warn!("Failed to send routing update: {}", e);
            } else {
                info!("✓ Sent UPDATE from {} with {} routes (seq: {})", 
                      router_id, update.routes.len(), current_seq);
            }
        }
    }

    async fn start_listen_task(&self) -> Result<(), Box<dyn std::error::Error>> {
        let socket = &self.socket;
        let router = self.router.clone();
        let neighbors = self.neighbors.clone();

        let mut buffer = [0u8; 4096];

        info!("Started listening for messages...");

        loop {
            match socket.recv_from(&mut buffer).await {
                Ok((len, addr)) => {
                    let data = String::from_utf8_lossy(&buffer[..len]);
                    debug!("Received {} bytes from {}: {}", len, addr, data);

                    if let Err(e) = self.handle_message(&data, addr, &router, &neighbors).await {
                        warn!("Failed to handle message from {}: {}", addr, e);
                    }
                }
                Err(e) => {
                    error!("Failed to receive message: {}", e);
                }
            }
        }
    }

    async fn handle_message(
        &self,
        data: &str,
        addr: SocketAddr,
        router: &Arc<Mutex<Router>>,
        neighbors: &Arc<Mutex<HashMap<String, (SocketAddr, RouterInfo)>>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(hello_data) = data.strip_prefix("HELLO:") {
            let hello: HelloMessage = serde_json::from_str(hello_data)?;

            // Ne pas traiter nos propres messages
            let router_guard = router.lock().await;
            if hello.router_id == router_guard.id {
                drop(router_guard);
                return Ok(());
            }
            drop(router_guard);

            info!("← Received HELLO from {} at {}", hello.router_id, addr);
            debug!("HELLO content: {:?}", hello);

            let router_info = RouterInfo {
                router_id: hello.router_id.clone(),
                interfaces: hello.interfaces.iter()
                    .map(|(name, ip)| {
                        let ip_addr: Ipv4Addr = ip.parse().unwrap_or(Ipv4Addr::new(0, 0, 0, 0));
                        (name.clone(), crate::network::InterfaceInfo {
                            name: name.clone(),
                            ip: ip_addr,
                            network: format!("{}/32", ip).parse().unwrap(),
                            metric: 1,
                        })
                    })
                    .collect(),
            };

            let mut neighbors_guard = neighbors.lock().await;
            neighbors_guard.insert(hello.router_id, (addr, router_info));
            info!("✓ Added/updated neighbor, total neighbors: {}", neighbors_guard.len());

        } else if let Some(update_data) = data.strip_prefix("UPDATE:") {
            let update: RoutingUpdate = serde_json::from_str(update_data)?;

            // Ne pas traiter nos propres messages
            let router_guard = router.lock().await;
            if update.router_id == router_guard.id {
                drop(router_guard);
                return Ok(());
            }
            drop(router_guard);

            info!("← Received UPDATE from {} with {} routes (seq: {})", 
                  update.router_id, update.routes.len(), update.sequence);
            debug!("UPDATE content: {:?}", update);

            self.process_routing_update(update, router).await?;
        }

        Ok(())
    }

    async fn process_routing_update(
        &self,
        update: RoutingUpdate,
        router: &Arc<Mutex<Router>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut new_routes = Vec::new();
        let mut processed_routes = 0;

        for route_info in update.routes {
            if let (Ok(destination), Ok(next_hop)) = (
                route_info.destination.parse(),
                route_info.next_hop.parse::<Ipv4Addr>(),
            ) {
                let router_guard = router.lock().await;

                // Éviter les routes vers nos propres réseaux
                let mut is_our_network = false;
                for (_, interface) in &router_guard.interfaces {
                    if interface.network == destination {
                        is_our_network = true;
                        break;
                    }
                }

                if !is_our_network {
                    // Choisir la première interface disponible
                    if let Some((_, interface)) = router_guard.interfaces.iter().next() {
                        let route = RouteEntry {
                            destination,
                            next_hop,
                            interface: interface.name.clone(),
                            metric: route_info.metric + 1,
                            source: RouteSource::Protocol,
                        };
                        new_routes.push(route);
                        processed_routes += 1;
                    }
                }
                drop(router_guard);
            }
        }

        if processed_routes > 0 {
            let mut router_guard = router.lock().await;
            router_guard.update_routing_table(new_routes)?;
            info!("✓ Updated routing table with {} new routes", processed_routes);
            drop(router_guard);
        }

        Ok(())
    }
}
