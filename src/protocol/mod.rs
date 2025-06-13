pub mod messages;
pub mod neighbor;
pub mod routing_table;

pub use messages::*;
pub use neighbor::*;
pub use routing_table::*;

use crate::{RouterId, NetworkId, SharedRouterState};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::interval;
use log::{info, warn, error, debug};

const HELLO_INTERVAL: Duration = Duration::from_secs(10);
const DEAD_INTERVAL: Duration = Duration::from_secs(40);
const PROTOCOL_PORT: u16 = 2089;

pub struct ProtocolEngine {
    state: SharedRouterState,
    socket: UdpSocket,
}

impl ProtocolEngine {
    pub async fn new(state: SharedRouterState) -> anyhow::Result<Self> {
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", PROTOCOL_PORT)).await?;
        socket.set_broadcast(true)?;

        Ok(Self { state, socket })
    }

    pub async fn start(&self) -> anyhow::Result<()> {
        let state_clone = self.state.clone();

        // Start hello timer
        tokio::spawn(async move {
            let mut hello_timer = interval(HELLO_INTERVAL);
            loop {
                hello_timer.tick().await;
                if let Err(e) = Self::send_hello_packets(&state_clone).await {
                    error!("Failed to send hello packets: {}", e);
                }
            }
        });

        // Start neighbor cleanup timer
        let state_clone = self.state.clone();
        tokio::spawn(async move {
            let mut cleanup_timer = interval(Duration::from_secs(5));
            loop {
                cleanup_timer.tick().await;
                Self::cleanup_dead_neighbors(&state_clone).await;
            }
        });

        // Start packet receiver
        self.receive_packets().await
    }

    async fn send_hello_packets(state: &SharedRouterState) -> anyhow::Result<()> {
        let router_state = state.read().await;

        if !router_state.enabled {
            return Ok(());
        }

        for (interface_name, interface) in &router_state.interfaces {
            if !interface.enabled {
                continue;
            }

            let hello = ProtocolMessage::Hello(HelloMessage {
                router_id: router_state.id.clone(),
                router_name: router_state.id.clone(),
                interface_ip: interface.ip_address,
                network: interface.network,
                bandwidth: interface.bandwidth,
                sequence: chrono::Utc::now().timestamp() as u32,
            });

            let data = bincode::serialize(&hello)?;
            let broadcast_addr = interface.network.broadcast();

            let socket = UdpSocket::bind("0.0.0.0:0").await?;
            socket.set_broadcast(true)?;

            if let Err(e) = socket.send_to(&data, format!("{}:{}", broadcast_addr, PROTOCOL_PORT)).await {
                warn!("Failed to send hello to {}: {}", broadcast_addr, e);
            } else {
                debug!("Sent hello on interface {} to {}", interface_name, broadcast_addr);
            }
        }

        Ok(())
    }

    async fn cleanup_dead_neighbors(state: &SharedRouterState) {
        let mut router_state = state.write().await;
        let now = chrono::Utc::now();

        let dead_neighbors: Vec<RouterId> = router_state
            .neighbors
            .iter()
            .filter(|(_, neighbor)| {
                now.signed_duration_since(neighbor.last_seen).num_seconds() > DEAD_INTERVAL.as_secs() as i64
            })
            .map(|(id, _)| id.clone())
            .collect();

        for neighbor_id in dead_neighbors {
            warn!("Neighbor {} is dead, removing", neighbor_id);
            router_state.neighbors.remove(&neighbor_id);
            router_state.topology.remove_neighbor(&neighbor_id);
        }

        if !router_state.neighbors.is_empty() {
            // Recalculate routes when topology changes
            drop(router_state);
            if let Err(e) = Self::calculate_routes(state).await {
                error!("Failed to recalculate routes: {}", e);
            }
        }
    }

    async fn receive_packets(&self) -> anyhow::Result<()> {
        let mut buf = [0u8; 1024];

        loop {
            match self.socket.recv_from(&mut buf).await {
                Ok((len, addr)) => {
                    if let Ok(message) = bincode::deserialize::<ProtocolMessage>(&buf[..len]) {
                        if let Err(e) = self.handle_message(message, addr).await {
                            error!("Failed to handle message from {}: {}", addr, e);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to receive packet: {}", e);
                }
            }
        }
    }

    async fn handle_message(&self, message: ProtocolMessage, from: SocketAddr) -> anyhow::Result<()> {
        match message {
            ProtocolMessage::Hello(hello) => {
                self.handle_hello(hello, from).await?;
            }
            ProtocolMessage::LSA(lsa) => {
                self.handle_lsa(lsa, from).await?;
            }
        }
        Ok(())
    }

    async fn handle_hello(&self, hello: HelloMessage, from: SocketAddr) -> anyhow::Result<()> {
        let mut router_state = self.state.write().await;

        if !router_state.enabled {
            return Ok(());
        }

        // Don't process our own hello messages
        if hello.router_id == router_state.id {
            return Ok(());
        }

        let neighbor = Neighbor {
            router_id: hello.router_id.clone(),
            router_name: hello.router_name,
            ip_address: hello.interface_ip,
            network: hello.network,
            bandwidth: hello.bandwidth,
            last_seen: chrono::Utc::now(),
            socket_addr: from,
        };

        let is_new_neighbor = !router_state.neighbors.contains_key(&hello.router_id);
        router_state.neighbors.insert(hello.router_id.clone(), neighbor.clone());
        router_state.topology.add_neighbor(neighbor);

        if is_new_neighbor {
            info!("New neighbor discovered: {} ({})", hello.router_id, hello.interface_ip);

            // Send our topology information to the new neighbor
            drop(router_state);
            self.send_lsa_to_neighbor(&hello.router_id).await?;

            // Recalculate routes
            Self::calculate_routes(&self.state).await?;
        }

        Ok(())
    }

    async fn handle_lsa(&self, lsa: LSAMessage, _from: SocketAddr) -> anyhow::Result<()> {
        let mut router_state = self.state.write().await;

        if !router_state.enabled {
            return Ok(());
        }

        // Update topology with LSA information
        router_state.topology.update_from_lsa(&lsa);

        drop(router_state);
        Self::calculate_routes(&self.state).await?;

        Ok(())
    }

    async fn send_lsa_to_neighbor(&self, neighbor_id: &str) -> anyhow::Result<()> {
        let router_state = self.state.read().await;

        if let Some(neighbor) = router_state.neighbors.get(neighbor_id) {
            let lsa = LSAMessage {
                router_id: router_state.id.clone(),
                sequence: chrono::Utc::now().timestamp() as u32,
                neighbors: router_state.neighbors.values().cloned().collect(),
                networks: router_state.interfaces.values()
                    .map(|iface| iface.network)
                    .collect(),
            };

            let message = ProtocolMessage::LSA(lsa);
            let data = bincode::serialize(&message)?;

            let socket = UdpSocket::bind("0.0.0.0:0").await?;
            socket.send_to(&data, neighbor.socket_addr).await?;

            debug!("Sent LSA to neighbor {}", neighbor_id);
        }

        Ok(())
    }

    pub async fn calculate_routes(state: &SharedRouterState) -> anyhow::Result<()> {
        use crate::algorithms::dijkstra::calculate_shortest_paths;

        let mut router_state = state.write().await;

        if !router_state.enabled {
            return Ok(());
        }

        // Calculate shortest paths using Dijkstra's algorithm
        let paths = calculate_shortest_paths(&router_state.topology, &router_state.id);

        // Update routing table
        router_state.routing_table.clear();

        for (dest_network, path) in paths {
            if let Some(next_hop) = path.next_hop {
                if let Some(neighbor) = router_state.neighbors.get(&next_hop) {
                    let route = RoutingEntry {
                        destination: dest_network,
                        next_hop: neighbor.ip_address,
                        interface: Self::find_interface_for_neighbor(&router_state.interfaces, neighbor),
                        metric: path.cost,
                        route_type: if router_state.is_default_router && dest_network == "0.0.0.0/0".parse().unwrap() {
                            RouteType::Default
                        } else {
                            RouteType::Internal
                        },
                    };

                    router_state.routing_table.add_route(route);
                }
            }
        }

        // Add default route if this router is configured as default
        if router_state.is_default_router {
            // Implementation would add system default route
            info!("Router {} is configured as default router", router_state.id);
        }

        info!("Routing table updated with {} routes", router_state.routing_table.len());

        Ok(())
    }

    fn find_interface_for_neighbor(interfaces: &HashMap<String, crate::network::NetworkInterface>, neighbor: &Neighbor) -> String {
        for (name, interface) in interfaces {
            if interface.network.contains(neighbor.ip_address) {
                return name.clone();
            }
        }
        "unknown".to_string()
    }
}
