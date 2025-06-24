use super::types::*;
use super::neighbor_manager;
use super::route_manager;
use super::SimpleRoutingProtocol;
use crate::router::RouterInfo;
use log::{info, warn, debug};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;

pub async fn handle_message(
    data: &str,
    addr: SocketAddr,
    protocol: &SimpleRoutingProtocol,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(hello_data) = data.strip_prefix("HELLO:") {
        handle_hello_message(hello_data, addr, protocol).await
    } else if let Some(update_data) = data.strip_prefix("UPDATE:") {
        handle_update_message(update_data, addr, protocol).await
    } else if let Some(request_data) = data.strip_prefix("NEIGHBOR_REQUEST:") {
        handle_neighbor_request(request_data, addr, protocol).await
    } else if let Some(response_data) = data.strip_prefix("NEIGHBOR_RESPONSE:") {
        handle_neighbor_response(response_data, protocol).await
    } else {
        debug!("Unknown message type from {}: {}", addr, data);
        Ok(())
    }
}

async fn handle_hello_message(
    hello_data: &str,
    addr: SocketAddr,
    protocol: &SimpleRoutingProtocol,
) -> Result<(), Box<dyn std::error::Error>> {
    let hello: HelloMessage = serde_json::from_str(hello_data)?;

    // Ignore messages from ourselves
    let router_guard = protocol.get_router().lock().await;
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

    let mut neighbors_guard = protocol.get_neighbors_arc().lock().await;
    let was_dead = neighbors_guard.get(&hello.router_id)
        .map(|n| !n.is_alive)
        .unwrap_or(true);

    neighbors_guard.insert(hello.router_id.clone(), neighbor_info);

    if was_dead {
        info!("🟢 Neighbor {} is now ALIVE at {}", hello.router_id, addr);
    }

    Ok(())
}

async fn handle_update_message(
    update_data: &str,
    addr: SocketAddr,
    protocol: &SimpleRoutingProtocol,
) -> Result<(), Box<dyn std::error::Error>> {
    let update: RoutingUpdate = serde_json::from_str(update_data)?;

    // Ignore messages from ourselves
    let router_guard = protocol.get_router().lock().await;
    if update.router_id == router_guard.id {
        drop(router_guard);
        return Ok(());
    }
    drop(router_guard);

    debug!("← Received UPDATE from {} with {} routes (seq: {})",
          update.router_id, update.routes.len(), update.sequence);

    // Update neighbor last seen time
    {
        let mut neighbors_guard = protocol.get_neighbors_arc().lock().await;
        if let Some(neighbor_info) = neighbors_guard.get_mut(&update.router_id) {
            neighbor_info.last_seen = Instant::now();
            if !neighbor_info.is_alive {
                info!("🟢 Neighbor {} is now ALIVE again (via UPDATE)", update.router_id);
                neighbor_info.is_alive = true;
            }
        }
    }

    route_manager::process_routing_update(
        update,
        addr,
        protocol.get_router(),
        protocol.get_route_states(),
    ).await.expect("Failed to process routing update");

    Ok(())
}

async fn handle_neighbor_request(
    request_data: &str,
    addr: SocketAddr,
    protocol: &SimpleRoutingProtocol,
) -> Result<(), Box<dyn std::error::Error>> {
    let request: NeighborRequest = serde_json::from_str(request_data)?;

    let router_guard = protocol.get_router().lock().await;
    let our_router_id = router_guard.id.clone();
    drop(router_guard);

    // Check if this request is for us
    if request.target_router_id == our_router_id {
        info!("← Received NEIGHBOR_REQUEST from {} for us (request_id: {})", 
               request.requesting_router_id, request.request_id);

        // Get our current neighbors
        let current_neighbors = neighbor_manager::get_current_neighbors_for_response(
            protocol.get_neighbors_arc()
        ).await;

        // Create response
        let neighbor_response = NeighborResponse {
            responding_router_id: our_router_id,
            request_id: request.request_id,
            neighbors: current_neighbors,
        };

        // Send response back
        send_neighbor_response(&neighbor_response, addr, protocol).await;
    } else {
        debug!("Received neighbor request for {} (not us: {})", request.target_router_id, our_router_id);
    }

    Ok(())
}

async fn handle_neighbor_response(
    response_data: &str,
    protocol: &SimpleRoutingProtocol,
) -> Result<(), Box<dyn std::error::Error>> {
    let response: NeighborResponse = serde_json::from_str(response_data)?;

    info!("← Received NEIGHBOR_RESPONSE from {} with {} neighbors (request_id: {})", 
           response.responding_router_id, response.neighbors.len(), response.request_id);

    // Check if we have a pending request for this response
    let mut pending_requests_guard = protocol.get_pending_neighbor_requests().lock().await;
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

    Ok(())
}

async fn send_neighbor_response(
    response: &NeighborResponse,
    addr: SocketAddr,
    protocol: &SimpleRoutingProtocol,
) {
    if let Ok(response_message) = serde_json::to_string(response) {
        let response_packet = format!("NEIGHBOR_RESPONSE:{}", response_message);

        if let Some(socket) = protocol.get_sockets().first() {
            if let Err(e) = socket.send_to(response_packet.as_bytes(), addr).await {
                warn!("Failed to send neighbor response to {}: {}", addr, e);
            } else {
                info!("→ Sent NEIGHBOR_RESPONSE to {} with {} neighbors)", 
                       addr, response.neighbors.len());
            }
        }
    }
}