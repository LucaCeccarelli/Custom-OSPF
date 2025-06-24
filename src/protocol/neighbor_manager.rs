use super::types::*;
use super::SimpleRoutingProtocol;
use crate::control_server::NeighborInfo as ControlNeighborInfo;
use log::{info, warn, debug};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, oneshot};

pub async fn get_neighbors_list(
    neighbors: &Arc<Mutex<HashMap<String, NeighborInfo>>>,
) -> Vec<ControlNeighborInfo> {
    let neighbors_guard = neighbors.lock().await;
    let mut neighbor_list = Vec::new();

    for (router_id, neighbor_info) in neighbors_guard.iter() {
        let last_seen = format!("{:?} ago", neighbor_info.last_seen.elapsed());
        let interfaces: Vec<String> = neighbor_info.router_info.interfaces
            .values()
            .map(|iface| iface.ip.to_string())
            .collect();

        neighbor_list.push(ControlNeighborInfo {
            router_id: router_id.clone(),
            ip_address: neighbor_info.socket_addr.ip().to_string(),
            last_seen,
            is_alive: neighbor_info.is_alive,
            interfaces,
        });
    }

    neighbor_list
}

pub async fn get_neighbors_of(
    protocol: &SimpleRoutingProtocol,
    router_id: &str,
) -> Option<Vec<ControlNeighborInfo>> {
    let router_guard = protocol.get_router().lock().await;
    let our_router_id = router_guard.id.clone();
    drop(router_guard);

    if router_id == our_router_id {
        // Return our own neighbors
        Some(get_neighbors_list(protocol.get_neighbors_arc()).await)
    } else {
        // Request neighbors from the specified router
        request_neighbors_from_router(protocol, router_id).await
    }
}

pub async fn request_neighbors_from_router(
    protocol: &SimpleRoutingProtocol,
    target_router_id: &str,
) -> Option<Vec<ControlNeighborInfo>> {
    // Generate a unique request ID
    let request_id = format!("{}_{}",
                             SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis(),
                             std::ptr::addr_of!(*protocol) as usize
    );

    let router_guard = protocol.get_router().lock().await;
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
        let mut pending_requests = protocol.get_pending_neighbor_requests().lock().await;
        pending_requests.insert(request_id.clone(), PendingNeighborRequest {
            responder: tx,
            timestamp: SystemTime::now(),
        });
    }

    info!("→ Requesting neighbors from router {} (request_id: {})", target_router_id, request_id);

    // Send the request as a broadcast
    send_neighbor_request(protocol, &neighbor_request).await;

    // Wait for response with timeout
    match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await {
        Ok(Ok(neighbors)) => {
            info!("✓ Received neighbor information for router {} ({} neighbors)", target_router_id, neighbors.len());
            Some(neighbors)
        }
        Ok(Err(_)) => {
            warn!("Channel closed while waiting for neighbor response from {}", target_router_id);
            cleanup_pending_request(protocol, &request_id).await;
            None
        }
        Err(_) => {
            warn!("Timeout waiting for neighbor response from router {}", target_router_id);
            cleanup_pending_request(protocol, &request_id).await;
            None
        }
    }
}

async fn send_neighbor_request(protocol: &SimpleRoutingProtocol, request: &NeighborRequest) {
    if let Ok(message) = serde_json::to_string(request) {
        let request_packet = format!("NEIGHBOR_REQUEST:{}", message);

        if let Some(socket) = protocol.get_sockets().first() {
            let router_guard = protocol.get_router().lock().await;
            for (_, interface) in &router_guard.interfaces {
                let broadcast_addr = interface.network.broadcast();
                let broadcast_target = format!("{}:{}", broadcast_addr, protocol.get_port());

                if let Ok(target_addr) = broadcast_target.parse::<SocketAddr>() {
                    if let Err(e) = socket.send_to(request_packet.as_bytes(), target_addr).await {
                        warn!("Failed to send neighbor request to {}: {}", target_addr, e);
                    } else {
                        debug!("✓ Sent NEIGHBOR_REQUEST to {} for router {}", target_addr, request.target_router_id);
                    }
                }
            }
            drop(router_guard);
        }
    }
}

async fn cleanup_pending_request(protocol: &SimpleRoutingProtocol, request_id: &str) {
    let mut pending_requests = protocol.get_pending_neighbor_requests().lock().await;
    pending_requests.remove(request_id);
}

pub async fn get_current_neighbors_for_response(
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