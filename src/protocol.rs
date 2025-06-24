use crate::network::Network;
use crate::router::{Router, RouterInfo};
use crate::routing_table::{RouteEntry, RouteSource};
use log::{info, warn, error, debug};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, oneshot};
use tokio::time::interval;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::broadcast;
use futures;

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

// New message types for neighbor requests
#[derive(Debug, Serialize, Deserialize)]
pub struct NeighborRequest {
    pub requesting_router_id: String,
    pub request_id: String,
    pub target_router_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NeighborResponse {
    pub responding_router_id: String,
    pub request_id: String,
    pub neighbors: Vec<NeighborResponseInfo>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NeighborResponseInfo {
    pub router_id: String,
    pub ip_address: String,
    pub last_seen: String,
    pub is_alive: bool,
    pub interfaces: Vec<String>,
}

#[derive(Debug)]
pub struct PendingNeighborRequest {
    pub responder: oneshot::Sender<Vec<crate::control_server::NeighborInfo>>,
    pub timestamp: SystemTime,
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
    route_states: Arc<Mutex<HashMap<String, RouteState>>>,
    sequence: Arc<Mutex<u64>>,
    port: u16,
    debug_mode: bool,
    our_ips: HashSet<Ipv4Addr>,
    is_running: Arc<AtomicBool>,
    shutdown_tx: Arc<Mutex<Option<broadcast::Sender<()>>>>,
    task_handles: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    pending_neighbor_requests: Arc<Mutex<HashMap<String, PendingNeighborRequest>>>,
}

impl SimpleRoutingProtocol {
    // Constants for timeouts
    const NEIGHBOR_TIMEOUT: Duration = Duration::from_secs(12);
    const ROUTE_TIMEOUT: Duration = Duration::from_secs(16);
    const HELLO_INTERVAL: Duration = Duration::from_secs(4);
    const UPDATE_INTERVAL: Duration = Duration::from_secs(8);
    const CLEANUP_INTERVAL: Duration = Duration::from_secs(5);

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
            is_running: Arc::new(AtomicBool::new(false)),
            shutdown_tx: Arc::new(Mutex::new(None)),
            task_handles: Arc::new(Mutex::new(Vec::new())),
            pending_neighbor_requests: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    // Add this new method to request neighbors from a specific router
    pub async fn request_neighbors_from_router(&self, target_router_id: &str) -> Option<Vec<crate::control_server::NeighborInfo>> {
        // Generate a unique request ID
        let request_id = format!("{}_{}",
                                 SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis(),
                                 std::ptr::addr_of!(*self) as usize // Use pointer address for uniqueness
        );

        let router_guard = self.router.lock().await;
        let our_router_id = router_guard.id.clone();
        drop(router_guard);

        // Create the neighbor request
        let neighbor_request = NeighborRequest {
            requesting_router_id: our_router_id,
            request_id: request_id.clone(),
            target_router_id: target_router_id.to_string(),
        };

        // Create a channel to receive the response
        let (tx, rx) = oneshot::channel();

        // Store the pending request
        {
            let mut pending_requests = self.pending_neighbor_requests.lock().await;
            pending_requests.insert(request_id.clone(), PendingNeighborRequest {
                responder: tx,
                timestamp: SystemTime::now(),
            });
        }

        info!("→ Requesting neighbors from router {} (request_id: {})", target_router_id, request_id);

        // Send the request as a broadcast
        if let Ok(message) = serde_json::to_string(&neighbor_request) {
            let request_packet = format!("NEIGHBOR_REQUEST:{}", message);

            if let Some(socket) = self.sockets.first() {
                let router_guard = self.router.lock().await;
                for (_, interface) in &router_guard.interfaces {
                    let broadcast_addr = interface.network.broadcast();
                    let broadcast_target = format!("{}:{}", broadcast_addr, self.port);

                    if let Ok(target_addr) = broadcast_target.parse::<SocketAddr>() {
                        if let Err(e) = socket.send_to(request_packet.as_bytes(), target_addr).await {
                            warn!("Failed to send neighbor request to {}: {}", target_addr, e);
                        } else {
                            debug!("✓ Sent NEIGHBOR_REQUEST to {} for router {}", target_addr, target_router_id);
                        }
                    }
                }
                drop(router_guard);
            }
        }

        // Wait for response with timeout
        match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(neighbors)) => {
                info!("✓ Received neighbor information for router {} ({} neighbors)", target_router_id, neighbors.len());
                Some(neighbors)
            }
            Ok(Err(_)) => {
                warn!("Channel closed while waiting for neighbor response from {}", target_router_id);
                // Clean up the pending request
                let mut pending_requests = self.pending_neighbor_requests.lock().await;
                pending_requests.remove(&request_id);
                None
            }
            Err(_) => {
                warn!("Timeout waiting for neighbor response from router {}", target_router_id);
                // Clean up the pending request
                let mut pending_requests = self.pending_neighbor_requests.lock().await;
                pending_requests.remove(&request_id);
                None
            }
        }
    }

    pub async fn get_status(&self) -> (String, bool) {
        let router_guard = self.router.lock().await;
        let router_id = router_guard.id.clone();
        drop(router_guard);

        let is_running = self.is_running.load(Ordering::Relaxed);
        (router_id, is_running)
    }

    pub async fn get_neighbors(&self) -> Vec<crate::control_server::NeighborInfo> {
        let neighbors_guard = self.neighbors.lock().await;
        let mut neighbor_list = Vec::new();

        for (router_id, neighbor_info) in neighbors_guard.iter() {
            let last_seen = format!("{:?} ago", neighbor_info.last_seen.elapsed());
            let interfaces: Vec<String> = neighbor_info.router_info.interfaces
                .values()
                .map(|iface| iface.ip.to_string())
                .collect();

            neighbor_list.push(crate::control_server::NeighborInfo {
                router_id: router_id.clone(),
                ip_address: neighbor_info.socket_addr.ip().to_string(),
                last_seen,
                is_alive: neighbor_info.is_alive,
                interfaces,
            });
        }

        neighbor_list
    }

    // Updated get_neighbors_of method
    pub async fn get_neighbors_of(&self, router_id: &str) -> Option<Vec<crate::control_server::NeighborInfo>> {
        let router_guard = self.router.lock().await;
        let our_router_id = router_guard.id.clone();
        drop(router_guard);

        if router_id == our_router_id {
            // Return our own neighbors
            Some(self.get_neighbors().await)
        } else {
            // Request neighbors from the specified router
            self.request_neighbors_from_router(router_id).await
        }
    }

    // Helper method to get current neighbors in response format
    async fn get_current_neighbors_static(
        neighbors: &Arc<Mutex<HashMap<String, NeighborInfo>>>,
    ) -> Vec<NeighborResponseInfo> {
        let neighbors_guard = neighbors.lock().await;
        let mut neighbor_list = Vec::new();

        for (router_id, neighbor_info) in neighbors_guard.iter() {
            let last_seen = format!("{:?} ago", neighbor_info.last_seen.elapsed());
            let interfaces: Vec<String> = neighbor_info.router_info.interfaces
                .values()
                .map(|iface| iface.ip.to_string())
                .collect();

            neighbor_list.push(NeighborResponseInfo {
                router_id: router_id.clone(),
                ip_address: neighbor_info.socket_addr.ip().to_string(),
                last_seen,
                is_alive: neighbor_info.is_alive,
                interfaces,
            });
        }

        neighbor_list
    }

    pub async fn start_protocol(&self) -> Result<(), Box<dyn std::error::Error>> {
        if self.is_running.load(Ordering::Relaxed) {
            return Ok(());
        }

        info!("Starting routing protocol...");
        self.is_running.store(true, Ordering::Relaxed);

        let (shutdown_tx, _) = broadcast::channel(1);
        {
            let mut tx_guard = self.shutdown_tx.lock().await;
            *tx_guard = Some(shutdown_tx);
        }

        self.start_tasks().await?;
        info!("✓ Routing protocol started successfully");
        Ok(())
    }

    pub async fn stop_protocol(&self) -> Result<(), Box<dyn std::error::Error>> {
        if !self.is_running.load(Ordering::Relaxed) {
            return Ok(());
        }

        info!("Stopping routing protocol...");
        self.is_running.store(false, Ordering::Relaxed);

        {
            let tx_guard = self.shutdown_tx.lock().await;
            if let Some(ref tx) = *tx_guard {
                let _ = tx.send(());
            }
        }

        {
            let mut handles_guard = self.task_handles.lock().await;
            for handle in handles_guard.drain(..) {
                handle.abort();
            }
        }

        info!("✓ Routing protocol stopped successfully");
        Ok(())
    }

    pub async fn start_tasks(&self) -> Result<(), Box<dyn std::error::Error>> {
        if !self.is_running.load(Ordering::Relaxed) {
            self.is_running.store(true, Ordering::Relaxed);
        }

        let mut handles_guard = self.task_handles.lock().await;

        for handle in handles_guard.drain(..) {
            handle.abort();
        }

        let shutdown_rx = {
            let tx_guard = self.shutdown_tx.lock().await;
            if let Some(ref tx) = *tx_guard {
                tx.subscribe()
            } else {
                let (tx, rx) = broadcast::channel(1);
                *self.shutdown_tx.lock().await = Some(tx);
                rx
            }
        };

        // Start hello task
        let hello_handle = {
            let router = self.router.clone();
            let sockets = self.sockets.clone();
            let port = self.port;
            let is_running = self.is_running.clone();
            let mut shutdown_rx = shutdown_rx.resubscribe();

            tokio::spawn(async move {
                Self::hello_task(router, sockets, port, is_running, &mut shutdown_rx).await;
            })
        };

        // Start update task
        let update_handle = {
            let router = self.router.clone();
            let sockets = self.sockets.clone();
            let sequence = self.sequence.clone();
            let port = self.port;
            let is_running = self.is_running.clone();
            let mut shutdown_rx = shutdown_rx.resubscribe();

            tokio::spawn(async move {
                Self::update_task(router, sockets, sequence, port, is_running, &mut shutdown_rx).await;
            })
        };

        // Start listen task with additional parameters for neighbor requests
        let listen_handle = {
            let router = self.router.clone();
            let neighbors = self.neighbors.clone();
            let route_states = self.route_states.clone();
            let sockets = self.sockets.clone();
            let our_ips = self.our_ips.clone();
            let is_running = self.is_running.clone();
            let pending_neighbor_requests = self.pending_neighbor_requests.clone();
            let port = self.port;
            let mut shutdown_rx = shutdown_rx.resubscribe();

            tokio::spawn(async move {
                Self::listen_task(router, neighbors, route_states, sockets, our_ips, is_running, pending_neighbor_requests, port, &mut shutdown_rx).await;
            })
        };

        // Start cleanup task
        let cleanup_handle = {
            let neighbors = self.neighbors.clone();
            let route_states = self.route_states.clone();
            let router = self.router.clone();
            let is_running = self.is_running.clone();
            let mut shutdown_rx = shutdown_rx.resubscribe();

            tokio::spawn(async move {
                Self::cleanup_task(neighbors, route_states, router, is_running, &mut shutdown_rx).await;
            })
        };

        // Start pending requests cleanup task
        let pending_cleanup_handle = {
            let pending_neighbor_requests = self.pending_neighbor_requests.clone();
            let is_running = self.is_running.clone();
            let mut shutdown_rx = shutdown_rx.resubscribe();

            tokio::spawn(async move {
                Self::cleanup_pending_requests_task(pending_neighbor_requests, is_running, &mut shutdown_rx).await;
            })
        };

        // Start debug task if enabled
        let debug_handle = if self.debug_mode {
            let router = self.router.clone();
            let neighbors = self.neighbors.clone();
            let is_running = self.is_running.clone();
            let mut shutdown_rx = shutdown_rx.resubscribe();

            Some(tokio::spawn(async move {
                Self::debug_task(router, neighbors, is_running, &mut shutdown_rx).await;
            }))
        } else {
            None
        };

        // Store handles
        handles_guard.push(hello_handle);
        handles_guard.push(update_handle);
        handles_guard.push(listen_handle);
        handles_guard.push(cleanup_handle);
        handles_guard.push(pending_cleanup_handle);

        if let Some(debug_handle) = debug_handle {
            handles_guard.push(debug_handle);
        }

        info!("✓ All protocol tasks started");
        Ok(())
    }

    pub async fn get_routing_table(&self) -> Vec<serde_json::Value> {
        let router_guard = self.router.lock().await;
        let routes = router_guard.routing_table.get_routes();

        routes.iter().map(|route| {
            serde_json::json!({
                "destination": route.destination.to_string(),
                "next_hop": if route.next_hop.is_unspecified() { 
                    "direct".to_string() 
                } else { 
                    route.next_hop.to_string() 
                },
                "interface": route.interface,
                "metric": route.metric,
                "source": format!("{:?}", route.source)
            })
        }).collect()
    }

    pub async fn start(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        info!("=== Starting routing protocol ===");
        self.print_initial_state().await;
        self.start_tasks().await
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

    // Static task methods with shutdown support
    async fn hello_task(
        router: Arc<Mutex<Router>>,
        sockets: Vec<Arc<UdpSocket>>,
        port: u16,
        is_running: Arc<AtomicBool>,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) {
        let mut interval = interval(Self::HELLO_INTERVAL);

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    debug!("Hello task shutting down");
                    break;
                }
                _ = interval.tick() => {
                    if !is_running.load(Ordering::Relaxed) {
                        break;
                    }

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

                    if let Ok(message) = serde_json::to_string(&hello) {
                        let hello_packet = format!("HELLO:{}", message);

                        if let Some(socket) = sockets.first() {
                            let router_guard = router.lock().await;
                            for (_, interface) in &router_guard.interfaces {
                                let broadcast_addr = interface.network.broadcast();
                                let broadcast_target = format!("{}:{}", broadcast_addr, port);

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
            }
        }
    }

    async fn update_task(
        router: Arc<Mutex<Router>>,
        sockets: Vec<Arc<UdpSocket>>,
        sequence: Arc<Mutex<u64>>,
        port: u16,
        is_running: Arc<AtomicBool>,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) {
        let mut interval = interval(Self::UPDATE_INTERVAL);

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    debug!("Update task shutting down");
                    break;
                }
                _ = interval.tick() => {
                    if !is_running.load(Ordering::Relaxed) {
                        break;
                    }

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
                            RouteInfo {
                                destination: route.destination.to_string(),
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

                    if let Ok(message) = serde_json::to_string(&update) {
                        let update_packet = format!("UPDATE:{}", message);

                        if let Some(socket) = sockets.first() {
                            let router_guard = router.lock().await;
                            for (_, interface) in &router_guard.interfaces {
                                let broadcast_addr = interface.network.broadcast();
                                let broadcast_target = format!("{}:{}", broadcast_addr, port);

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
            }
        }
    }

    // Updated listen task to handle neighbor requests
    async fn listen_task(
        router: Arc<Mutex<Router>>,
        neighbors: Arc<Mutex<HashMap<String, NeighborInfo>>>,
        route_states: Arc<Mutex<HashMap<String, RouteState>>>,
        sockets: Vec<Arc<UdpSocket>>,
        our_ips: HashSet<Ipv4Addr>,
        is_running: Arc<AtomicBool>,
        pending_neighbor_requests: Arc<Mutex<HashMap<String, PendingNeighborRequest>>>,
        port: u16,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) {
        let mut tasks = Vec::new();

        for (i, socket) in sockets.iter().enumerate() {
            let socket_clone = socket.clone();
            let router_clone = router.clone();
            let neighbors_clone = neighbors.clone();
            let route_states_clone = route_states.clone();
            let our_ips_clone = our_ips.clone();
            let is_running_clone = is_running.clone();
            let pending_neighbor_requests_clone = pending_neighbor_requests.clone();
            let sockets_clone = sockets.clone();
            let mut shutdown_rx_clone = shutdown_rx.resubscribe();

            let task = tokio::spawn(async move {
                let mut buffer = [0u8; 4096];

                loop {
                    tokio::select! {
                        _ = shutdown_rx_clone.recv() => {
                            debug!("Listen task {} shutting down", i);
                            break;
                        }
                        result = socket_clone.recv_from(&mut buffer) => {
                            if !is_running_clone.load(Ordering::Relaxed) {
                                break;
                            }

                            match result {
                                Ok((len, addr)) => {
                                    if let std::net::IpAddr::V4(ipv4_addr) = addr.ip() {
                                        if our_ips_clone.contains(&ipv4_addr) {
                                            debug!("Ignoring loopback message from our own IP: {}", addr.ip());
                                            continue;
                                        }
                                    }

                                    let data = String::from_utf8_lossy(&buffer[..len]);
                                    debug!("Socket {} received {} bytes from {}", i, len, addr);

                                    if let Err(e) = Self::handle_message_static(
                                        &data, 
                                        addr, 
                                        &router_clone, 
                                        &neighbors_clone, 
                                        &route_states_clone,
                                        &pending_neighbor_requests_clone,
                                        &sockets_clone,
                                        port
                                    ).await {
                                        warn!("Failed to handle message from {}: {}", addr, e);
                                    }
                                }
                                Err(e) => {
                                    error!("Socket {} failed to receive message: {}", i, e);
                                    tokio::time::sleep(Duration::from_millis(100)).await;
                                }
                            }
                        }
                    }
                }
            });

            tasks.push(task);
        }

        futures::future::join_all(tasks).await;
    }

    async fn cleanup_task(
        neighbors: Arc<Mutex<HashMap<String, NeighborInfo>>>,
        route_states: Arc<Mutex<HashMap<String, RouteState>>>,
        router: Arc<Mutex<Router>>,
        is_running: Arc<AtomicBool>,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) {
        let mut interval = interval(Self::CLEANUP_INTERVAL);

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    debug!("Cleanup task shutting down");
                    break;
                }
                _ = interval.tick() => {
                    if !is_running.load(Ordering::Relaxed) {
                        break;
                    }

                    let now = Instant::now();
                    let mut dead_neighbors = Vec::new();
                    let mut stale_routes = Vec::new();

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

                    {
                        let route_states_guard = route_states.lock().await;
                        for (destination, route_state) in route_states_guard.iter() {
                            if now.duration_since(route_state.last_advertised) > Self::ROUTE_TIMEOUT {
                                stale_routes.push(destination.clone());
                            }
                        }
                    }

                    if !dead_neighbors.is_empty() || !stale_routes.is_empty() {
                        let mut routes_to_remove = HashSet::new();

                        {
                            let route_states_guard = route_states.lock().await;
                            for (destination, route_state) in route_states_guard.iter() {
                                if dead_neighbors.contains(&route_state.advertising_neighbor) ||
                                   stale_routes.contains(destination) {
                                    routes_to_remove.insert(destination.clone());
                                }
                            }
                        }

                        if !routes_to_remove.is_empty() {
                            Self::remove_routes_static(routes_to_remove, &router, &route_states).await;
                        }
                    }

                    for dead_neighbor_id in &dead_neighbors {
                        if let Some(dead_neighbor_ip) = Self::get_neighbor_ip_static(dead_neighbor_id, &neighbors).await {
                            let mut router_guard = router.lock().await;
                            if let Ok(removed_routes) = router_guard.routing_table.remove_routes_via_nexthop(dead_neighbor_ip).await {
                                if !removed_routes.is_empty() {
                                    info!("V Successfully removed {} routes via dead neighbor {}", 
                                          removed_routes.len(), dead_neighbor_ip);
                                }
                            }
                            drop(router_guard);
                        }
                    }
                }
            }
        }
    }

    // New cleanup task for pending neighbor requests
    async fn cleanup_pending_requests_task(
        pending_requests: Arc<Mutex<HashMap<String, PendingNeighborRequest>>>,
        is_running: Arc<AtomicBool>,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) {
        let mut interval = interval(Duration::from_secs(10));

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    debug!("Pending requests cleanup task shutting down");
                    break;
                }
                _ = interval.tick() => {
                    if !is_running.load(Ordering::Relaxed) {
                        break;
                    }

                    let now = SystemTime::now();
                    let mut expired_requests = Vec::new();

                    {
                        let pending_requests_guard = pending_requests.lock().await;
                        for (request_id, request) in pending_requests_guard.iter() {
                            if now.duration_since(request.timestamp).unwrap_or(Duration::ZERO) > Duration::from_secs(30) {
                                expired_requests.push(request_id.clone());
                            }
                        }
                    }

                    if !expired_requests.is_empty() {
                        let mut pending_requests_guard = pending_requests.lock().await;
                        for request_id in expired_requests {
                            pending_requests_guard.remove(&request_id);
                            debug!("Cleaned up expired neighbor request: {}", request_id);
                        }
                    }
                }
            }
        }
    }

    async fn debug_task(
        router: Arc<Mutex<Router>>,
        neighbors: Arc<Mutex<HashMap<String, NeighborInfo>>>,
        is_running: Arc<AtomicBool>,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) {
        let mut interval = interval(Duration::from_secs(30));

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    debug!("Debug task shutting down");
                    break;
                }
                _ = interval.tick() => {
                    if !is_running.load(Ordering::Relaxed) {
                        break;
                    }

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
        }
    }

    // Helper static methods
    async fn get_neighbor_ip_static(
        neighbor_id: &str,
        neighbors: &Arc<Mutex<HashMap<String, NeighborInfo>>>,
    ) -> Option<Ipv4Addr> {
        let neighbors_guard = neighbors.lock().await;
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

    async fn remove_routes_static(
        destinations: HashSet<String>,
        router: &Arc<Mutex<Router>>,
        route_states: &Arc<Mutex<HashMap<String, RouteState>>>,
    ) {
        let mut router_guard = router.lock().await;
        let mut route_states_guard = route_states.lock().await;

        for destination in destinations {
            if let Some(route_state) = route_states_guard.remove(&destination) {
                info!("Removing route to {} (was via {})",
                      destination, route_state.advertising_neighbor);

                if let Err(e) = router_guard.routing_table.remove_route(&route_state.route).await {
                    warn!("Failed to remove route to {}: {}", destination, e);
                } else {
                    info!("V Successfully removed route to {} from system", destination);
                }
            }
        }
    }

    async fn get_neighbor_ip(&self, neighbor_id: &str) -> Option<Ipv4Addr> {
        Self::get_neighbor_ip_static(neighbor_id, &self.neighbors).await
    }

    async fn remove_routes(&self, destinations: HashSet<String>) -> Result<(), Box<dyn std::error::Error>> {
        Self::remove_routes_static(destinations, &self.router, &self.route_states).await;
        Ok(())
    }

    // Updated handle_message_static method to handle neighbor requests and responses
    async fn handle_message_static(
        data: &str,
        addr: SocketAddr,
        router: &Arc<Mutex<Router>>,
        neighbors: &Arc<Mutex<HashMap<String, NeighborInfo>>>,
        route_states: &Arc<Mutex<HashMap<String, RouteState>>>,
        pending_neighbor_requests: &Arc<Mutex<HashMap<String, PendingNeighborRequest>>>,
        sockets: &[Arc<UdpSocket>],
        port: u16,
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

        } else if let Some(request_data) = data.strip_prefix("NEIGHBOR_REQUEST:") {
            let request: NeighborRequest = serde_json::from_str(request_data)?;

            let router_guard = router.lock().await;
            let our_router_id = router_guard.id.clone();
            drop(router_guard);

            // Check if this request is for us
            if request.target_router_id == our_router_id {
                info!("← Received NEIGHBOR_REQUEST from {} for us (request_id: {})", 
                       request.requesting_router_id, request.request_id);

                // Get our current neighbors
                let current_neighbors = Self::get_current_neighbors_static(neighbors).await;

                // Create response
                let neighbor_response = NeighborResponse {
                    responding_router_id: our_router_id,
                    request_id: request.request_id,
                    neighbors: current_neighbors,
                };

                // Send response back
                if let Ok(response_message) = serde_json::to_string(&neighbor_response) {
                    let response_packet = format!("NEIGHBOR_RESPONSE:{}", response_message);

                    if let Some(socket) = sockets.first() {
                        if let Err(e) = socket.send_to(response_packet.as_bytes(), addr).await {
                            warn!("Failed to send neighbor response to {}: {}", addr, e);
                        } else {
                            info!("→ Sent NEIGHBOR_RESPONSE to {} with {} neighbors)", 
                                   addr, neighbor_response.neighbors.len());
                        }
                    }
                }
            } else {
                debug!("Received neighbor request for {} (not us: {})", request.target_router_id, our_router_id);
            }

        } else if let Some(response_data) = data.strip_prefix("NEIGHBOR_RESPONSE:") {
            let response: NeighborResponse = serde_json::from_str(response_data)?;

            info!("← Received NEIGHBOR_RESPONSE from {} with {} neighbors (request_id: {})", 
                   response.responding_router_id, response.neighbors.len(), response.request_id);

            // Check if we have a pending request for this response
            let mut pending_requests_guard = pending_neighbor_requests.lock().await;
            if let Some(pending_request) = pending_requests_guard.remove(&response.request_id) {
                // Convert response format to our internal format
                let neighbors: Vec<crate::control_server::NeighborInfo> = response.neighbors
                    .into_iter()
                    .map(|n| crate::control_server::NeighborInfo {
                        router_id: n.router_id,
                        ip_address: n.ip_address,
                        last_seen: n.last_seen,
                        is_alive: n.is_alive,
                        interfaces: n.interfaces,
                    })
                    .collect();

                // Send the response through the channel
                if let Err(_) = pending_request.responder.send(neighbors) {
                    warn!("Failed to send neighbor response through channel (request_id: {})", 
                          response.request_id);
                } else {
                    info!("✓ Successfully delivered neighbor information for router {} (request_id: {})",
                          response.responding_router_id, response.request_id);
                }
            } else {
                debug!("Received neighbor response for unknown request_id: {}", response.request_id);
            }
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