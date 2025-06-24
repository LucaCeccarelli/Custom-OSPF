use super::types::*;
use super::{neighbor_manager, route_manager};
use super::SimpleRoutingProtocol;
use crate::router::Router;
use log::{info, warn, error, debug};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, broadcast};
use tokio::time::interval;
use futures;

pub async fn start_tasks(protocol: &SimpleRoutingProtocol) -> Result<(), Box<dyn std::error::Error>> {
    if !protocol.is_running().load(Ordering::Relaxed) {
        protocol.is_running().store(true, Ordering::Relaxed);
    }

    let mut handles_guard = protocol.get_task_handles().lock().await;

    for handle in handles_guard.drain(..) {
        handle.abort();
    }

    let shutdown_rx = {
        let tx_guard = protocol.get_shutdown_tx().lock().await;
        if let Some(ref tx) = *tx_guard {
            tx.subscribe()
        } else {
            let (tx, rx) = broadcast::channel(1);
            *protocol.get_shutdown_tx().lock().await = Some(tx);
            rx
        }
    };

    // Start hello task
    let hello_handle = start_hello_task(protocol, shutdown_rx.resubscribe()).await;

    // Start update task
    let update_handle = start_update_task(protocol, shutdown_rx.resubscribe()).await;

    // Start listen task
    let listen_handle = start_listen_task(protocol, shutdown_rx.resubscribe()).await;

    // Start cleanup task
    let cleanup_handle = start_cleanup_task(protocol, shutdown_rx.resubscribe()).await;

    // Start pending requests cleanup task
    let pending_cleanup_handle = start_pending_cleanup_task(protocol, shutdown_rx.resubscribe()).await;

    // Start debug task if enabled
    let debug_handle = if protocol.is_debug_mode() {
        Some(start_debug_task(protocol, shutdown_rx.resubscribe()).await)
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

    info!("All protocol tasks started");
    Ok(())
}

async fn start_hello_task(
    protocol: &SimpleRoutingProtocol,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    let router = protocol.get_router().clone();
    let sockets = protocol.get_sockets().to_vec();
    let port = protocol.get_port();
    let is_running = protocol.is_running().clone();

    tokio::spawn(async move {
        hello_task(router, sockets, port, is_running, &mut shutdown_rx).await;
    })
}

async fn start_update_task(
    protocol: &SimpleRoutingProtocol,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    let router = protocol.get_router().clone();
    let sockets = protocol.get_sockets().to_vec();
    let sequence = protocol.get_sequence().clone();
    let port = protocol.get_port();
    let is_running = protocol.is_running().clone();

    tokio::spawn(async move {
        update_task(router, sockets, sequence, port, is_running, &mut shutdown_rx).await;
    })
}

async fn start_listen_task(
    protocol: &SimpleRoutingProtocol,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    let router = protocol.get_router().clone();
    let neighbors = protocol.get_neighbors_arc().clone();
    let route_states = protocol.get_route_states().clone();
    let sockets = protocol.get_sockets().to_vec();
    let our_ips = protocol.get_our_ips().clone();
    let is_running = protocol.is_running().clone();
    let pending_neighbor_requests = protocol.get_pending_neighbor_requests().clone();
    let port = protocol.get_port();

    tokio::spawn(async move {
        listen_task(
            router,
            neighbors,
            route_states,
            sockets,
            our_ips,
            is_running,
            pending_neighbor_requests,
            port,
            &mut shutdown_rx,
        ).await;
    })
}

async fn start_cleanup_task(
    protocol: &SimpleRoutingProtocol,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    let neighbors = protocol.get_neighbors_arc().clone();
    let route_states = protocol.get_route_states().clone();
    let router = protocol.get_router().clone();
    let is_running = protocol.is_running().clone();

    tokio::spawn(async move {
        cleanup_task(neighbors, route_states, router, is_running, &mut shutdown_rx).await;
    })
}

async fn start_pending_cleanup_task(
    protocol: &SimpleRoutingProtocol,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    let pending_neighbor_requests = protocol.get_pending_neighbor_requests().clone();
    let is_running = protocol.is_running().clone();

    tokio::spawn(async move {
        cleanup_pending_requests_task(pending_neighbor_requests, is_running, &mut shutdown_rx).await;
    })
}

async fn start_debug_task(
    protocol: &SimpleRoutingProtocol,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    let router = protocol.get_router().clone();
    let neighbors = protocol.get_neighbors_arc().clone();
    let is_running = protocol.is_running().clone();

    tokio::spawn(async move {
        debug_task(router, neighbors, is_running, &mut shutdown_rx).await;
    })
}

async fn hello_task(
    router: Arc<Mutex<Router>>,
    sockets: Vec<Arc<UdpSocket>>,
    port: u16,
    is_running: Arc<std::sync::atomic::AtomicBool>,
    shutdown_rx: &mut broadcast::Receiver<()>,
) {
    let mut interval = interval(SimpleRoutingProtocol::HELLO_INTERVAL);

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

                send_hello_messages(&router, &sockets, port).await;
            }
        }
    }
}

async fn send_hello_messages(
    router: &Arc<Mutex<Router>>,
    sockets: &[Arc<UdpSocket>],
    port: u16,
) {
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
                        debug!("Sent HELLO from {} to {}", interface.ip, target_addr);
                    }
                }
            }
            drop(router_guard);
        }
    }
}

async fn update_task(
    router: Arc<Mutex<Router>>,
    sockets: Vec<Arc<UdpSocket>>,
    sequence: Arc<Mutex<u64>>,
    port: u16,
    is_running: Arc<std::sync::atomic::AtomicBool>,
    shutdown_rx: &mut broadcast::Receiver<()>,
) {
    let mut interval = interval(SimpleRoutingProtocol::UPDATE_INTERVAL);

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

                send_routing_updates(&router, &sockets, &sequence, port).await;
            }
        }
    }
}

async fn send_routing_updates(
    router: &Arc<Mutex<Router>>,
    sockets: &[Arc<UdpSocket>],
    sequence: &Arc<Mutex<u64>>,
    port: u16,
) {
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
                        debug!("Sent UPDATE from {} to {} with {} routes (seq: {})",
                              interface.ip, target_addr, update.routes.len(), current_seq);
                    }
                }
            }
            drop(router_guard);
        }
    }
}

async fn listen_task(
    router: Arc<Mutex<Router>>,
    neighbors: Arc<Mutex<HashMap<String, NeighborInfo>>>,
    route_states: Arc<Mutex<HashMap<String, RouteState>>>,
    sockets: Vec<Arc<UdpSocket>>,
    our_ips: HashSet<Ipv4Addr>,
    is_running: Arc<std::sync::atomic::AtomicBool>,
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
            socket_listen_loop(
                i,
                socket_clone,
                router_clone,
                neighbors_clone,
                route_states_clone,
                our_ips_clone,
                is_running_clone,
                pending_neighbor_requests_clone,
                sockets_clone,
                port,
                &mut shutdown_rx_clone,
            ).await;
        });

        tasks.push(task);
    }

    futures::future::join_all(tasks).await;
}

async fn socket_listen_loop(
    socket_id: usize,
    socket: Arc<UdpSocket>,
    router: Arc<Mutex<Router>>,
    neighbors: Arc<Mutex<HashMap<String, NeighborInfo>>>,
    route_states: Arc<Mutex<HashMap<String, RouteState>>>,
    our_ips: HashSet<Ipv4Addr>,
    is_running: Arc<std::sync::atomic::AtomicBool>,
    pending_neighbor_requests: Arc<Mutex<HashMap<String, PendingNeighborRequest>>>,
    sockets: Vec<Arc<UdpSocket>>,
    port: u16,
    shutdown_rx: &mut broadcast::Receiver<()>,
) {
    let mut buffer = [0u8; 4096];

    // Create a simplified protocol wrapper for message handling
    let protocol_data = ProtocolData {
        router,
        neighbors,
        route_states,
        pending_neighbor_requests,
        sockets,
        port,
    };

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                debug!("Listen task {} shutting down", socket_id);
                break;
            }
            result = socket.recv_from(&mut buffer) => {
                if !is_running.load(Ordering::Relaxed) {
                    break;
                }

                match result {
                    Ok((len, addr)) => {
                        if let std::net::IpAddr::V4(ipv4_addr) = addr.ip() {
                            if our_ips.contains(&ipv4_addr) {
                                debug!("Ignoring loopback message from our own IP: {}", addr.ip());
                                continue;
                            }
                        }

                        let data = String::from_utf8_lossy(&buffer[..len]);
                        debug!("Socket {} received {} bytes from {}", socket_id, len, addr);

                        if let Err(e) = handle_message_with_protocol_data(&data, addr, &protocol_data).await {
                            warn!("Failed to handle message from {}: {}", addr, e);
                        }
                    }
                    Err(e) => {
                        error!("Socket {} failed to receive message: {}", socket_id, e);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        }
    }
}

// Helper struct to pass protocol data to message handler
struct ProtocolData {
    router: Arc<Mutex<Router>>,
    neighbors: Arc<Mutex<HashMap<String, NeighborInfo>>>,
    route_states: Arc<Mutex<HashMap<String, RouteState>>>,
    pending_neighbor_requests: Arc<Mutex<HashMap<String, PendingNeighborRequest>>>,
    sockets: Vec<Arc<UdpSocket>>,
    port: u16,
}

async fn handle_message_with_protocol_data(
    data: &str,
    addr: SocketAddr,
    protocol_data: &ProtocolData,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(hello_data) = data.strip_prefix("HELLO:") {
        handle_hello_message_static(hello_data, addr, protocol_data).await
    } else if let Some(update_data) = data.strip_prefix("UPDATE:") {
        handle_update_message_static(update_data, addr, protocol_data).await
    } else if let Some(request_data) = data.strip_prefix("NEIGHBOR_REQUEST:") {
        handle_neighbor_request_static(request_data, addr, protocol_data).await
    } else if let Some(response_data) = data.strip_prefix("NEIGHBOR_RESPONSE:") {
        handle_neighbor_response_static(response_data, protocol_data).await
    } else {
        debug!("Unknown message type from {}: {}", addr, data);
        Ok(())
    }
}

async fn handle_hello_message_static(
    hello_data: &str,
    addr: SocketAddr,
    protocol_data: &ProtocolData,
) -> Result<(), Box<dyn std::error::Error>> {
    let hello: HelloMessage = serde_json::from_str(hello_data)?;

    // Ignore messages from ourselves
    let router_guard = protocol_data.router.lock().await;
    if hello.router_id == router_guard.id {
        drop(router_guard);
        return Ok(());
    }
    drop(router_guard);

    debug!("Received HELLO from {} at {}", hello.router_id, addr);

    let router_info = crate::router::RouterInfo {
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

    let mut neighbors_guard = protocol_data.neighbors.lock().await;
    let was_dead = neighbors_guard.get(&hello.router_id)
        .map(|n| !n.is_alive)
        .unwrap_or(true);

    neighbors_guard.insert(hello.router_id.clone(), neighbor_info);

    if was_dead {
        info!("Neighbor {} is now ALIVE at {}", hello.router_id, addr);
    }

    Ok(())
}

async fn handle_update_message_static(
    update_data: &str,
    addr: SocketAddr,
    protocol_data: &ProtocolData,
) -> Result<(), Box<dyn std::error::Error>> {
    let update: RoutingUpdate = serde_json::from_str(update_data)?;

    // Ignore messages from ourselves
    let router_guard = protocol_data.router.lock().await;
    if update.router_id == router_guard.id {
        drop(router_guard);
        return Ok(());
    }
    drop(router_guard);

    debug!("Received UPDATE from {} with {} routes (seq: {})",
          update.router_id, update.routes.len(), update.sequence);

    // Update neighbor last seen time
    {
        let mut neighbors_guard = protocol_data.neighbors.lock().await;
        if let Some(neighbor_info) = neighbors_guard.get_mut(&update.router_id) {
            neighbor_info.last_seen = Instant::now();
            if !neighbor_info.is_alive {
                info!("Neighbor {} is now ALIVE again (via UPDATE)", update.router_id);
                neighbor_info.is_alive = true;
            }
        }
    }

    route_manager::process_routing_update(
        update,
        addr,
        &protocol_data.router,
        &protocol_data.route_states,
    ).await.expect("Failed to process routing update");

    Ok(())
}

async fn handle_neighbor_request_static(
    request_data: &str,
    addr: SocketAddr,
    protocol_data: &ProtocolData,
) -> Result<(), Box<dyn std::error::Error>> {
    let request: NeighborRequest = serde_json::from_str(request_data)?;

    let router_guard = protocol_data.router.lock().await;
    let our_router_id = router_guard.id.clone();
    drop(router_guard);

    // Check if this request is for us
    if request.target_router_id == our_router_id {
        info!("Received NEIGHBOR_REQUEST from {} for us (request_id: {})",
               request.requesting_router_id, request.request_id);

        // Get our current neighbors
        let current_neighbors = neighbor_manager::get_current_neighbors_for_response(
            &protocol_data.neighbors
        ).await;

        // Create response
        let neighbor_response = NeighborResponse {
            responding_router_id: our_router_id,
            request_id: request.request_id,
            neighbors: current_neighbors,
        };

        // Send response back
        if let Ok(response_message) = serde_json::to_string(&neighbor_response) {
            let response_packet = format!("NEIGHBOR_RESPONSE:{}", response_message);

            if let Some(socket) = protocol_data.sockets.first() {
                if let Err(e) = socket.send_to(response_packet.as_bytes(), addr).await {
                    warn!("Failed to send neighbor response to {}: {}", addr, e);
                } else {
                    info!("Sent NEIGHBOR_RESPONSE to {} with {} neighbors)",
                           addr, neighbor_response.neighbors.len());
                }
            }
        }
    } else {
        debug!("Received neighbor request for {} (not us: {})", request.target_router_id, our_router_id);
    }

    Ok(())
}

async fn handle_neighbor_response_static(
    response_data: &str,
    protocol_data: &ProtocolData,
) -> Result<(), Box<dyn std::error::Error>> {
    let response: NeighborResponse = serde_json::from_str(response_data)?;

    info!("Received NEIGHBOR_RESPONSE from {} with {} neighbors (request_id: {})",
           response.responding_router_id, response.neighbors.len(), response.request_id);

    // Check if we have a pending request for this response
    let mut pending_requests_guard = protocol_data.pending_neighbor_requests.lock().await;
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
            info!("Successfully delivered neighbor information for router {} (request_id: {})",
                  response.responding_router_id, response.request_id);
        }
    } else {
        debug!("Received neighbor response for unknown request_id: {}", response.request_id);
    }

    Ok(())
}

async fn cleanup_task(
    neighbors: Arc<Mutex<HashMap<String, NeighborInfo>>>,
    route_states: Arc<Mutex<HashMap<String, RouteState>>>,
    router: Arc<Mutex<Router>>,
    is_running: Arc<std::sync::atomic::AtomicBool>,
    shutdown_rx: &mut broadcast::Receiver<()>,
) {
    let mut interval = interval(SimpleRoutingProtocol::CLEANUP_INTERVAL);

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

                perform_cleanup(&neighbors, &route_states, &router).await;
            }
        }
    }
}

async fn perform_cleanup(
    neighbors: &Arc<Mutex<HashMap<String, NeighborInfo>>>,
    route_states: &Arc<Mutex<HashMap<String, RouteState>>>,
    router: &Arc<Mutex<Router>>,
) {
    let now = Instant::now();
    let mut dead_neighbors = Vec::new();
    let mut stale_routes = Vec::new();

    // Check for dead neighbors and stale routes
    {
        let mut neighbors_guard = neighbors.lock().await;
        for (neighbor_id, neighbor_info) in neighbors_guard.iter_mut() {
            if now.duration_since(neighbor_info.last_seen) > SimpleRoutingProtocol::NEIGHBOR_TIMEOUT {
                if neighbor_info.is_alive {
                    warn!("Neighbor {} is now considered DEAD (last seen: {:?} ago)",
                          neighbor_id, now.duration_since(neighbor_info.last_seen));
                    neighbor_info.is_alive = false;
                    dead_neighbors.push(neighbor_id.clone());
                }
            } else if !neighbor_info.is_alive {
                info!("Neighbor {} is now ALIVE again", neighbor_id);
                neighbor_info.is_alive = true;
            }
        }
    }

    {
        let route_states_guard = route_states.lock().await;
        for (destination, route_state) in route_states_guard.iter() {
            if now.duration_since(route_state.last_advertised) > SimpleRoutingProtocol::ROUTE_TIMEOUT {
                stale_routes.push(destination.clone());
            }
        }
    }

    // Remove routes for dead neighbors and stale routes
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
            route_manager::remove_routes_via_nexthop(routes_to_remove, router, route_states).await;
        }
    }

    // Remove routes via dead neighbor IPs
    for dead_neighbor_id in &dead_neighbors {
        if let Some(dead_neighbor_ip) = route_manager::get_neighbor_ip(dead_neighbor_id, neighbors).await {
            let mut router_guard = router.lock().await;
            if let Ok(removed_routes) = router_guard.routing_table.remove_routes_via_nexthop(dead_neighbor_ip).await {
                if !removed_routes.is_empty() {
                    info!("Successfully removed {} routes via dead neighbor {}",
                          removed_routes.len(), dead_neighbor_ip);
                }
            }
            drop(router_guard);
        }
    }
}

async fn cleanup_pending_requests_task(
    pending_requests: Arc<Mutex<HashMap<String, PendingNeighborRequest>>>,
    is_running: Arc<std::sync::atomic::AtomicBool>,
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

                cleanup_expired_requests(&pending_requests).await;
            }
        }
    }
}

async fn cleanup_expired_requests(
    pending_requests: &Arc<Mutex<HashMap<String, PendingNeighborRequest>>>,
) {
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

async fn debug_task(
    router: Arc<Mutex<Router>>,
    neighbors: Arc<Mutex<HashMap<String, NeighborInfo>>>,
    is_running: Arc<std::sync::atomic::AtomicBool>,
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

                print_debug_status(&router, &neighbors).await;
            }
        }
    }
}

async fn print_debug_status(
    router: &Arc<Mutex<Router>>,
    neighbors: &Arc<Mutex<HashMap<String, NeighborInfo>>>,
) {
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