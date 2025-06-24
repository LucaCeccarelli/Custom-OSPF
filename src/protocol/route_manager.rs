use super::types::*;
use crate::routing_table::{RouteEntry, RouteSource};
use crate::router::Router;
use log::{info, warn, debug};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

pub async fn process_routing_update(
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

    let mut new_routes = Vec::new();
    let mut updated_route_states = Vec::new();
    let now = Instant::now();

    let router_guard = router.lock().await;
    let router_interfaces = router_guard.interfaces.clone();
    drop(router_guard);

    for route_info in update.routes {
        if let Some((route, route_state)) = process_single_route(
            &route_info,
            &update.router_id,
            sender_ip,
            &router_interfaces,
            now,
        ).await? {
            // Check if we should replace an existing route
            let should_add = should_accept_route(&route, router).await;

            if should_add {
                info!("Accepting route to {} via {} metric {} from {}",
                      route.destination, route.next_hop, route.metric, update.router_id);
                new_routes.push(route);
                updated_route_states.push((route_info.destination.clone(), route_state));
            } else {
                debug!("Keeping existing route to {} (not better)", route.destination);
            }
        }
    }

    // Update route states
    update_route_states(route_states, updated_route_states).await;

    // Apply new routes
    if !new_routes.is_empty() {
        apply_new_routes(router, new_routes, &update.router_id).await.expect("Failed to apply new routes");
    }

    Ok(())
}

async fn process_single_route(
    route_info: &RouteInfo,
    advertising_router: &str,
    sender_ip: Ipv4Addr,
    router_interfaces: &HashMap<String, crate::network::InterfaceInfo>,
    now: Instant,
) -> Result<Option<(RouteEntry, RouteState)>, Box<dyn std::error::Error + Send + Sync>> {
    let corrected_destination = match host_to_network_route(&route_info.destination) {
        Ok(dest) => dest,
        Err(e) => {
            warn!("Failed to correct route destination {}: {}", route_info.destination, e);
            return Ok(None);
        }
    };

    let destination = match corrected_destination.parse::<ipnetwork::Ipv4Network>() {
        Ok(dest) => dest,
        Err(_) => return Ok(None),
    };

    let advertised_next_hop = match route_info.next_hop.parse::<Ipv4Addr>() {
        Ok(nh) => nh,
        Err(_) => return Ok(None),
    };

    // Check if this is our own network
    if is_our_network(&destination, router_interfaces) {
        return Ok(None);
    }

    // Find best interface to reach sender
    let (interface_name, _interface_info) = match find_best_interface_for_sender(sender_ip, router_interfaces) {
        Some(iface) => iface,
        None => return Ok(None),
    };

    // Determine actual next hop
    let actual_next_hop = determine_next_hop(advertised_next_hop, sender_ip, router_interfaces);

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
        advertising_neighbor: advertising_router.to_string(),
    };

    Ok(Some((route, route_state)))
}

fn host_to_network_route(host_with_prefix: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let network: ipnetwork::Ipv4Network = host_with_prefix.parse()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
    let network_addr = network.network();
    let prefix_len = network.prefix();
    Ok(format!("{}/{}", network_addr, prefix_len))
}

fn is_our_network(
    destination: &ipnetwork::Ipv4Network,
    router_interfaces: &HashMap<String, crate::network::InterfaceInfo>,
) -> bool {
    for (_, interface) in router_interfaces {
        if interface.network == *destination {
            return true;
        }
    }
    false
}

fn find_best_interface_for_sender(
    sender_ip: Ipv4Addr,
    router_interfaces: &HashMap<String, crate::network::InterfaceInfo>,
) -> Option<(String, crate::network::InterfaceInfo)> {
    let mut best_interface = None;
    let mut best_metric = u32::MAX;

    for (name, interface) in router_interfaces {
        if interface.network.contains(sender_ip) {
            if interface.metric < best_metric {
                best_interface = Some((name.clone(), interface.clone()));
                best_metric = interface.metric;
            }
        }
    }

    best_interface
}

fn determine_next_hop(
    advertised_next_hop: Ipv4Addr,
    sender_ip: Ipv4Addr,
    router_interfaces: &HashMap<String, crate::network::InterfaceInfo>,
) -> Ipv4Addr {
    if advertised_next_hop.is_unspecified() {
        return sender_ip;
    }

    // Check if advertised next hop is reachable through our interfaces
    for (_, local_interface) in router_interfaces {
        if local_interface.network.contains(advertised_next_hop) {
            return advertised_next_hop;
        }
    }

    // Use sender IP as next hop
    sender_ip
}

async fn should_accept_route(route: &RouteEntry, router: &Arc<Mutex<Router>>) -> bool {
    let router_guard = router.lock().await;
    let existing_route = router_guard.routing_table.find_route(&route.destination);

    match existing_route {
        Some(existing) => {
            // Replace if new route has better metric, or same metric but from different neighbor
            route.metric < existing.metric ||
                (route.metric == existing.metric && route.next_hop != existing.next_hop)
        },
        None => true,
    }
}

async fn update_route_states(
    route_states: &Arc<Mutex<HashMap<String, RouteState>>>,
    updated_route_states: Vec<(String, RouteState)>,
) {
    let mut route_states_guard = route_states.lock().await;
    for (destination, route_state) in updated_route_states {
        route_states_guard.insert(destination, route_state);
    }
}

async fn apply_new_routes(
    router: &Arc<Mutex<Router>>,
    new_routes: Vec<RouteEntry>,
    advertising_router: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut router_guard = router.lock().await;
    router_guard.update_routing_table(new_routes).await?;
    info!("✓ Updated routing table with routes from {}", advertising_router);
    Ok(())
}

pub async fn remove_routes_via_nexthop(
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

pub async fn get_neighbor_ip(
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