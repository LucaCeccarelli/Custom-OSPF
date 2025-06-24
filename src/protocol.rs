use crate::network::Network;
use crate::router::{Router, RouterInfo};
use crate::routing_table::{RouteEntry, RouteSource};
use log::{info, warn, error, debug};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
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

#[derive(Debug, Clone)]
pub struct NeighborInfo {
    pub router_info: RouterInfo,
    pub last_seen: Instant,
    pub socket_addr: SocketAddr,
    pub is_alive: bool,
}

#[derive(Debug, Clone)]
pub struct RouteState {
    pub route: RouteEntry,
    pub last_advertised: Instant,
    pub advertising_neighbor: String,
}

pub struct SimpleRoutingProtocol {
    router: Arc<Mutex<Router>>,
    sockets: Vec<Arc<UdpSocket>>,
    neighbors: Arc<Mutex<HashMap<String, NeighborInfo>>>,
    route_states: Arc<Mutex<HashMap<String, RouteState>>>, // destination -> route state
    sequence: Arc<Mutex<u64>>,
    port: u16,
    debug_mode: bool,
    our_ips: HashSet<Ipv4Addr>,
}

impl SimpleRoutingProtocol {
    // Constants for timeouts
    const NEIGHBOR_TIMEOUT: Duration = Duration::from_secs(12); // 3 * hello interval
    const ROUTE_TIMEOUT: Duration = Duration::from_secs(16);    // 2 * update interval
    const HELLO_INTERVAL: Duration = Duration::from_secs(4);    // Fast neighbor detection
    const UPDATE_INTERVAL: Duration = Duration::from_secs(8);   // Quick route convergence
    const CLEANUP_INTERVAL: Duration = Duration::from_secs(5);  // Frequent cleanup

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
        ).await?;

        let mut sockets = Vec::new();
        let mut our_ips = HashSet::new();

        let bind_addr = format!("0.0.0.0:{}", port);
        match UdpSocket::bind(&bind_addr).await {
            Ok(socket) => {
                socket.set_broadcast(true)?;
                info!("✓ Socket bound to {} for all interfaces", bind_addr);
                sockets.push(Arc::new(socket));

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
            route_states: Arc::new(Mutex::new(HashMap::new())),
            sequence: Arc::new(Mutex::new(0)),
            port,
            debug_mode,
            our_ips,
        })
    }

    pub async fn start(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        info!("=== Starting routing protocol ===");

        self.print_initial_state().await;

        let hello_task = self.start_hello_task();
        let update_task = self.start_update_task();
        let listen_task = self.start_listen_task();
        let cleanup_task = self.start_cleanup_task(); // New cleanup task
        let debug_task = if self.debug_mode {
            Some(self.start_debug_task())
        } else {
            None
        };

        if let Some(debug_task) = debug_task {
            tokio::try_join!(hello_task, update_task, listen_task, cleanup_task, debug_task)?;
        } else {
            tokio::try_join!(hello_task, update_task, listen_task, cleanup_task)?;
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

    // New cleanup task to handle dead neighbors and stale routes
    async fn start_cleanup_task(&self) -> Result<(), Box<dyn std::error::Error>> {
        let neighbors = self.neighbors.clone();
        let route_states = self.route_states.clone();
        let router = self.router.clone();

        let mut interval = interval(Self::CLEANUP_INTERVAL);

        loop {
            interval.tick().await;

            let now = Instant::now();
            let mut dead_neighbors = Vec::new();
            let mut stale_routes = Vec::new();

            // Check for dead neighbors
            {
                let mut neighbors_guard = neighbors.lock().await;
                for (neighbor_id, neighbor_info) in neighbors_guard.iter_mut() {
                    if now.duration_since(neighbor_info.last_seen) > Self::NEIGHBOR_TIMEOUT {
                        if neighbor_info.is_alive {
                            warn!("X Neighbor {} is now considered DEAD (last seen: {:?} ago)",
                                  neighbor_id, now.duration_since(neighbor_info.last_seen));
                            neighbor_info.is_alive = false;
                            dead_neighbors.push(neighbor_id.clone());
                        }
                    } else if !neighbor_info.is_alive {
                        info!("O Neighbor {} is now ALIVE again", neighbor_id);
                        neighbor_info.is_alive = true;
                    }
                }
            }

            // Check for stale routes
            {
                let route_states_guard = route_states.lock().await;
                for (destination, route_state) in route_states_guard.iter() {
                    if now.duration_since(route_state.last_advertised) > Self::ROUTE_TIMEOUT {
                        warn!("Route to {} is stale (last advertised: {:?} ago)",
                              destination, now.duration_since(route_state.last_advertised));
                        stale_routes.push(destination.clone());
                    }
                }
            }

            // Remove routes from dead neighbors and stale routes
            if !dead_neighbors.is_empty() || !stale_routes.is_empty() {
                let mut routes_to_remove = HashSet::new();

                {
                    let route_states_guard = route_states.lock().await;
                    for (destination, route_state) in route_states_guard.iter() {
                        // Remove routes from dead neighbors
                        if dead_neighbors.contains(&route_state.advertising_neighbor) {
                            routes_to_remove.insert(destination.clone());
                            info!("Marking route to {} for removal (dead neighbor: {})",
                                  destination, route_state.advertising_neighbor);
                        }
                        // Remove stale routes
                        if stale_routes.contains(destination) {
                            routes_to_remove.insert(destination.clone());
                            info!("Marking route to {} for removal (stale route)", destination);
                        }
                    }
                }

                if !routes_to_remove.is_empty() {
                    info!("Cleaning up {} stale/dead routes", routes_to_remove.len());
                    self.remove_routes(routes_to_remove).await?;
                }
            }

            // Also remove routes directly from routing table by dead neighbor IP
            for dead_neighbor_id in &dead_neighbors {
                if let Some(dead_neighbor_ip) = self.get_neighbor_ip(dead_neighbor_id).await {
                    info!("X Removing routes via dead neighbor IP: {}", dead_neighbor_ip);
                    let mut router_guard = router.lock().await;
                    let removed_routes = router_guard.routing_table.remove_routes_via_nexthop(dead_neighbor_ip).await?;
                    if !removed_routes.is_empty() {
                        info!("V Successfully removed {} routes via dead neighbor {}",
                              removed_routes.len(), dead_neighbor_ip);
                    }
                    drop(router_guard);
                }
            }

            // Clean up completely dead neighbors after some time
            {
                let mut neighbors_guard = neighbors.lock().await;
                neighbors_guard.retain(|neighbor_id, neighbor_info| {
                    if !neighbor_info.is_alive &&
                        now.duration_since(neighbor_info.last_seen) > Self::NEIGHBOR_TIMEOUT * 2 {
                        info!("Removing dead neighbor {} from neighbor table", neighbor_id);
                        false
                    } else {
                        true
                    }
                });
            }
        }
    }

    // Helper method to get neighbor IP from neighbor ID
    async fn get_neighbor_ip(&self, neighbor_id: &str) -> Option<Ipv4Addr> {
        let neighbors_guard = self.neighbors.lock().await;
        if let Some(neighbor_info) = neighbors_guard.get(neighbor_id) {
            if let std::net::IpAddr::V4(ipv4) = neighbor_info.socket_addr.ip() {
                Some(ipv4)
            } else {
                None
            }
        } else {
            None
        }
    }

    async fn remove_routes(&self, destinations: HashSet<String>) -> Result<(), Box<dyn std::error::Error>> {
        let mut router_guard = self.router.lock().await;
        let mut route_states_guard = self.route_states.lock().await;

        for destination in destinations {
            if let Some(route_state) = route_states_guard.remove(&destination) {
                info!("Removing route to {} (was via {})",
                      destination, route_state.advertising_neighbor);

                // Remove from system routing table
                if let Err(e) = router_guard.routing_table.remove_route(&route_state.route).await {
                    warn!("Failed to remove route to {}: {}", destination, e);
                } else {
                    info!("V Successfully removed route to {} from system", destination);
                }
            }
        }

        Ok(())
    }

    async fn start_debug_task(&self) -> Result<(), Box<dyn std::error::Error>> {
        let router = self.router.clone();
        let neighbors = self.neighbors.clone();

        let mut interval = interval(Duration::from_secs(30));

        loop {
            interval.tick().await;

            info!("=== DEBUG STATUS ===");

            let neighbors_guard = neighbors.lock().await;
            info!("Neighbors ({}): ", neighbors_guard.len());
            for (id, neighbor_info) in neighbors_guard.iter() {
                let status = if neighbor_info.is_alive { "O ALIVE" } else { "X DEAD" };
                let last_seen = neighbor_info.last_seen.elapsed();
                info!("  {} at {} - {} (last seen: {:?} ago)",
                      id, neighbor_info.socket_addr, status, last_seen);
            }
            drop(neighbors_guard);

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
        let mut interval = interval(Self::HELLO_INTERVAL);

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

            if let Some(socket) = self.sockets.first() {
                let router_guard = router.lock().await;
                for (_, interface) in &router_guard.interfaces {
                    let broadcast_addr = interface.network.broadcast();
                    let broadcast_target = format!("{}:{}", broadcast_addr, self.port);

                    if let Ok(target_addr) = broadcast_target.parse::<SocketAddr>() {
                        if let Err(e) = socket.send_to(hello_packet.as_bytes(), target_addr).await {
                            warn!("Failed to send hello from {} to {}: {}", interface.ip, target_addr, e);
                        } else {
                            debug!("✓ Sent HELLO from {} to {}", interface.ip, target_addr);
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
        let mut interval = interval(Self::UPDATE_INTERVAL);

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
                .map(|route| {
                    let destination_str = route.destination.to_string();

                    RouteInfo {
                        destination: destination_str,
                        metric: route.metric,
                        next_hop: route.next_hop.to_string(),
                    }
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

            if let Some(socket) = self.sockets.first() {
                let router_guard = router.lock().await;
                for (_, interface) in &router_guard.interfaces {
                    let broadcast_addr = interface.network.broadcast();
                    let broadcast_target = format!("{}:{}", broadcast_addr, self.port);

                    if let Ok(target_addr) = broadcast_target.parse::<SocketAddr>() {
                        if let Err(e) = socket.send_to(update_packet.as_bytes(), target_addr).await {
                            warn!("Failed to send update from {} to {}: {}", interface.ip, target_addr, e);
                        } else {
                            debug!("✓ Sent UPDATE from {} to {} with {} routes (seq: {})",
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
        let route_states = self.route_states.clone();

        info!("Started listening for messages on {} sockets...", self.sockets.len());

        let mut tasks = Vec::new();

        for (i, socket) in self.sockets.iter().enumerate() {
            let socket_clone = socket.clone();
            let router_clone = router.clone();
            let neighbors_clone = neighbors.clone();
            let route_states_clone = route_states.clone();
            let our_ips_clone = self.our_ips.clone();

            let task = tokio::spawn(async move {
                let mut buffer = [0u8; 4096];

                loop {
                    match socket_clone.recv_from(&mut buffer).await {
                        Ok((len, addr)) => {
                            if let std::net::IpAddr::V4(ipv4_addr) = addr.ip() {
                                if our_ips_clone.contains(&ipv4_addr) {
                                    debug!("Ignoring loopback message from our own IP: {}", addr.ip());
                                    continue;
                                }
                            }

                            let data = String::from_utf8_lossy(&buffer[..len]);
                            debug!("Socket {} received {} bytes from {}", i, len, addr);

                            if let Err(e) = Self::handle_message_static(&data, addr, &router_clone, &neighbors_clone, &route_states_clone).await {
                                warn!("Failed to handle message from {}: {}", addr, e);
                            }
                        }
                        Err(e) => {
                            error!("Socket {} failed to receive message: {}", i, e);
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

    async fn handle_message_static(
        data: &str,
        addr: SocketAddr,
        router: &Arc<Mutex<Router>>,
        neighbors: &Arc<Mutex<HashMap<String, NeighborInfo>>>,
        route_states: &Arc<Mutex<HashMap<String, RouteState>>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(hello_data) = data.strip_prefix("HELLO:") {
            let hello: HelloMessage = serde_json::from_str(hello_data)?;

            let router_guard = router.lock().await;
            if hello.router_id == router_guard.id {
                drop(router_guard);
                return Ok(());
            }
            drop(router_guard);

            debug!("← Received HELLO from {} at {}", hello.router_id, addr);

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

            let neighbor_info = NeighborInfo {
                router_info,
                last_seen: Instant::now(),
                socket_addr: addr,
                is_alive: true,
            };

            let mut neighbors_guard = neighbors.lock().await;
            let was_dead = neighbors_guard.get(&hello.router_id)
                .map(|n| !n.is_alive)
                .unwrap_or(true);

            neighbors_guard.insert(hello.router_id.clone(), neighbor_info);

            if was_dead {
                info!("🟢 Neighbor {} is now ALIVE at {}", hello.router_id, addr);
            }

        } else if let Some(update_data) = data.strip_prefix("UPDATE:") {
            let update: RoutingUpdate = serde_json::from_str(update_data)?;

            let router_guard = router.lock().await;
            if update.router_id == router_guard.id {
                drop(router_guard);
                return Ok(());
            }
            drop(router_guard);

            debug!("← Received UPDATE from {} with {} routes (seq: {})",
                  update.router_id, update.routes.len(), update.sequence);

            // Update neighbor's last seen time
            {
                let mut neighbors_guard = neighbors.lock().await;
                if let Some(neighbor_info) = neighbors_guard.get_mut(&update.router_id) {
                    neighbor_info.last_seen = Instant::now();
                    if !neighbor_info.is_alive {
                        info!("🟢 Neighbor {} is now ALIVE again (via UPDATE)", update.router_id);
                        neighbor_info.is_alive = true;
                    }
                }
            }

            Self::process_routing_update_static(update, addr, router, route_states).await.expect("Failed to process routing update");
        }

        Ok(())
    }

    async fn process_routing_update_static(
        update: RoutingUpdate,
        sender_addr: SocketAddr,
        router: &Arc<Mutex<Router>>,
        route_states: &Arc<Mutex<HashMap<String, RouteState>>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let sender_ip = if let std::net::IpAddr::V4(ipv4) = sender_addr.ip() {
            ipv4
        } else {
            return Err("Only IPv4 addresses supported".into());
        };

        fn host_to_network_route(host_with_prefix: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
            let network: ipnetwork::Ipv4Network = host_with_prefix.parse()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            let network_addr = network.network();
            let prefix_len = network.prefix();
            Ok(format!("{}/{}", network_addr, prefix_len))
        }

        let mut new_routes = Vec::new();
        let mut updated_route_states = Vec::new();
        let now = Instant::now();

        let router_guard = router.lock().await;
        let router_interfaces = router_guard.interfaces.clone();
        drop(router_guard);

        for route_info in update.routes {
            let corrected_destination = match host_to_network_route(&route_info.destination) {
                Ok(dest) => dest,
                Err(e) => {
                    warn!("Failed to correct route destination {}: {}", route_info.destination, e);
                    continue;
                }
            };

            if let Ok(destination) = corrected_destination.parse::<ipnetwork::Ipv4Network>() {
                if let Ok(advertised_next_hop) = route_info.next_hop.parse::<Ipv4Addr>() {
                    let mut is_our_network = false;
                    for (_, interface) in &router_interfaces {
                        if interface.network == destination {
                            is_our_network = true;
                            break;
                        }
                    }

                    if is_our_network {
                        continue;
                    }

                    let mut best_interface = None;
                    let mut best_metric = u32::MAX;

                    for (name, interface) in &router_interfaces {
                        if interface.network.contains(sender_ip) {
                            if interface.metric < best_metric {
                                best_interface = Some((name.clone(), interface.clone()));
                                best_metric = interface.metric;
                            }
                        }
                    }

                    if let Some((interface_name, _interface_info)) = best_interface {
                        let actual_next_hop = if advertised_next_hop.is_unspecified() {
                            sender_ip
                        } else {
                            let mut use_advertised = false;
                            for (_, local_interface) in &router_interfaces {
                                if local_interface.network.contains(advertised_next_hop) {
                                    use_advertised = true;
                                    break;
                                }
                            }

                            if use_advertised {
                                advertised_next_hop
                            } else {
                                sender_ip
                            }
                        };

                        let route = RouteEntry {
                            destination,
                            next_hop: actual_next_hop,
                            interface: interface_name,
                            metric: route_info.metric + 1,
                            source: RouteSource::Protocol,
                        };

                        let route_state = RouteState {
                            route: route.clone(),
                            last_advertised: now,
                            advertising_neighbor: update.router_id.clone(),
                        };

                        // Check if we should replace an existing route
                        let should_add = {
                            let router_guard = router.lock().await;
                            let existing_route = router_guard.routing_table.find_route(&destination);

                            match existing_route {
                                Some(existing) => {
                                    // Replace if new route has better metric, or same metric but from alive neighbor
                                    route.metric < existing.metric ||
                                        (route.metric == existing.metric && route.next_hop != existing.next_hop)
                                },
                                None => true,
                            }
                        };

                        if should_add {
                            info!("Accepting route to {} via {} metric {} from {}",
                                  destination, actual_next_hop, route.metric, update.router_id);
                            new_routes.push(route);
                            updated_route_states.push((corrected_destination.clone(), route_state));
                        } else {
                            debug!("Keeping existing route to {} (not better)", destination);
                        }
                    }
                }
            }
        }

        // Update route states
        {
            let mut route_states_guard = route_states.lock().await;
            for (destination, route_state) in updated_route_states {
                route_states_guard.insert(destination, route_state);
            }
        }

        if !new_routes.is_empty() {
            let mut router_guard = router.lock().await;
            router_guard.update_routing_table(new_routes).await.expect("Failed to update routing table");
            info!("✓ Updated routing table with routes from {}", update.router_id);
        }

        Ok(())
    }
}