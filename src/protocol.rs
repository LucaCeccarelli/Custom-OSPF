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
    sockets: Vec<Arc<UdpSocket>>,
    neighbors: Arc<Mutex<HashMap<String, (SocketAddr, RouterInfo)>>>,
    sequence: Arc<Mutex<u64>>,
    port: u16,
    debug_mode: bool,
    our_ips: HashSet<Ipv4Addr>,
}

impl SimpleRoutingProtocol {
    pub async fn new(
        router_id: String,
        interface_names: HashSet<String>,
        port: u16,
        network: Network,
        debug_mode: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let router = Router::new(
            router_id,
            network,
            interface_names.into_iter().collect(),
        )?;

        // Create sockets - bind to 0.0.0.0 to receive broadcasts
        let mut sockets = Vec::new();
        let mut our_ips = HashSet::new();

        // Option 1: Single socket approach (recommended)
        let bind_addr = format!("0.0.0.0:{}", port);
        match UdpSocket::bind(&bind_addr).await {
            Ok(socket) => {
                socket.set_broadcast(true)?;

                // Additional socket options for better broadcast handling
                //let socket2 = socket2::Socket::from(std::os::unix::io::AsRawFd::as_raw_fd(&socket));
                //socket2.set_reuse_address(true)?;

                info!("✓ Socket bound to {} for all interfaces", bind_addr);
                sockets.push(Arc::new(socket));

                // Collect all our IPs
                for (_, interface) in &router.interfaces {
                    our_ips.insert(interface.ip);
                }
            }
            Err(e) => {
                return Err(format!("Failed to bind socket: {}", e).into());
            }
        }

        if sockets.is_empty() {
            return Err("No sockets could be created".into());
        }

        info!("Router created successfully");
        info!("Bound to {} sockets on port {}", sockets.len(), port);
        info!("Our IPs: {:?}", our_ips);

        Ok(Self {
            router: Arc::new(Mutex::new(router)),
            sockets,
            neighbors: Arc::new(Mutex::new(HashMap::new())),
            sequence: Arc::new(Mutex::new(0)),
            port,
            debug_mode,
            our_ips,
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
        let router = self.router.clone();

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

            // Envoyer depuis chaque socket vers le broadcast de son réseau
            for (i, socket) in self.sockets.iter().enumerate() {
                let router_guard = router.lock().await;
                if let Some((_, interface)) = router_guard.interfaces.iter().nth(i) {
                    // Calculer l'adresse de broadcast pour ce réseau
                    let broadcast_addr = interface.network.broadcast();
                    let broadcast_target = format!("{}:{}", broadcast_addr, self.port);

                    if let Ok(target_addr) = broadcast_target.parse::<SocketAddr>() {
                        if let Err(e) = socket.send_to(hello_packet.as_bytes(), target_addr).await {
                            warn!("Failed to send hello from {} to {}: {}", interface.ip, target_addr, e);
                        } else {
                            info!("✓ Sent HELLO from {} to {}", interface.ip, target_addr);
                        }
                    }
                }
                drop(router_guard);
            }
        }
    }

    async fn start_update_task(&self) -> Result<(), Box<dyn std::error::Error>> {
        let router = self.router.clone();
        let sequence = self.sequence.clone();

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

            // Envoyer depuis chaque socket vers le broadcast de son réseau
            for (i, socket) in self.sockets.iter().enumerate() {
                let router_guard = router.lock().await;
                if let Some((_, interface)) = router_guard.interfaces.iter().nth(i) {
                    let broadcast_addr = interface.network.broadcast();
                    let broadcast_target = format!("{}:{}", broadcast_addr, self.port);

                    if let Ok(target_addr) = broadcast_target.parse::<SocketAddr>() {
                        if let Err(e) = socket.send_to(update_packet.as_bytes(), target_addr).await {
                            warn!("Failed to send update from {} to {}: {}", interface.ip, target_addr, e);
                        } else {
                            info!("✓ Sent UPDATE from {} to {} with {} routes (seq: {})",
                                  interface.ip, target_addr, update.routes.len(), current_seq);
                        }
                    }
                }
                drop(router_guard);
            }
        }
    }

    async fn start_listen_task(&self) -> Result<(), Box<dyn std::error::Error>> {
        let router = self.router.clone();
        let neighbors = self.neighbors.clone();

        info!("Started listening for messages on {} sockets...", self.sockets.len());

        let mut tasks = Vec::new();

        for (i, socket) in self.sockets.iter().enumerate() {
            let socket_clone = socket.clone();
            let router_clone = router.clone();
            let neighbors_clone = neighbors.clone();
            let our_ips_clone = self.our_ips.clone();

            let task = tokio::spawn(async move {
                let mut buffer = [0u8; 4096];

                loop {
                    match socket_clone.recv_from(&mut buffer).await {
                        Ok((len, addr)) => {
                            // Filter loopback messages
                            if let std::net::IpAddr::V4(ipv4_addr) = addr.ip() {
                                if our_ips_clone.contains(&ipv4_addr) {
                                    debug!("Ignoring loopback message from our own IP: {}", addr.ip());
                                    continue;
                                }
                            }

                            let data = String::from_utf8_lossy(&buffer[..len]);
                            debug!("Socket {} received {} bytes from {}: {}", i, len, addr, data);

                            if let Err(e) = Self::handle_message_static(&data, addr, &router_clone, &neighbors_clone).await {
                                warn!("Failed to handle message from {}: {}", addr, e);
                            }
                        }
                        Err(e) => {
                            error!("Socket {} failed to receive message: {}", i, e);
                            // Add a small delay to prevent tight error loops
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                }
            });

            tasks.push(task);
        }

        futures::future::join_all(tasks).await;
        Ok(())
    }


    // Version statique de handle_message pour être utilisée dans les tâches async
    async fn handle_message_static(
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
                debug!("Ignoring our own HELLO message");
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
                debug!("Ignoring our own UPDATE message");
                return Ok(());
            }
            drop(router_guard);

            info!("← Received UPDATE from {} with {} routes (seq: {})",
                  update.router_id, update.routes.len(), update.sequence);
            debug!("UPDATE content: {:?}", update);

            Self::process_routing_update_static(update, router).await?;
        }

        Ok(())
    }

    // Version statique de process_routing_update
    async fn process_routing_update_static(
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

    async fn handle_message(
        &self,
        data: &str,
        addr: SocketAddr,
        router: &Arc<Mutex<Router>>,
        neighbors: &Arc<Mutex<HashMap<String, (SocketAddr, RouterInfo)>>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        Self::handle_message_static(data, addr, router, neighbors).await
    }

    async fn process_routing_update(
        &self,
        update: RoutingUpdate,
        router: &Arc<Mutex<Router>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        Self::process_routing_update_static(update, router).await
    }
}