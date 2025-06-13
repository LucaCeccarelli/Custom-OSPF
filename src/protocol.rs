use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tokio::net::UdpSocket;
use tokio::time::{interval, Duration};
use tracing::{info, warn, error, debug};
use chrono::Utc;

use crate::message::{ProtocolMessage, HelloMessage, LSAMessage, LinkData, LSAData, RouterLSAData, LSAType};
use crate::neighbor::{Neighbor, NeighborManager, NeighborState};
use crate::network::{NetworkTopology, NetworkNode};
use crate::routing_table::{RoutingTable, RoutingEntry, RouteType};
use crate::config::{RouterConfig, InterfaceConfig};

pub struct RoutingProtocol {
    pub router_id: String,
    pub router_name: String,
    pub config: RouterConfig,
    pub enabled: bool,
    pub is_default_router: bool,

    // Network state
    pub neighbor_manager: NeighborManager,
    pub topology: NetworkTopology,
    pub routing_table: RoutingTable,
    pub lsa_database: HashMap<String, LSAMessage>,

    // Sequence numbers
    pub sequence_number: u32,

    // Communication
    socket: Arc<UdpSocket>,

    // Channels for communication with router
    pub control_rx: mpsc::Receiver<ProtocolControl>,
    control_tx: mpsc::Sender<ProtocolControl>,
}

#[derive(Debug)]
pub enum ProtocolControl {
    Enable,
    Disable,
    Recalculate,
    InterfaceUp(String),
    InterfaceDown(String),
    SetDefaultRouter(bool),
}

impl RoutingProtocol {
    pub async fn new(
        router_id: String,
        router_name: String,
        config: RouterConfig,
        bind_addr: SocketAddr,
        is_default_router: bool,
    ) -> anyhow::Result<Self> {
        let socket = Arc::new(UdpSocket::bind(bind_addr).await?);
        let (control_tx, control_rx) = mpsc::channel(100);

        Ok(Self {
            router_id: router_id.clone(),
            router_name,
            config: config.clone(),
            enabled: true,
            is_default_router,

            neighbor_manager: NeighborManager::new(config.dead_interval),
            topology: NetworkTopology::new(),
            routing_table: RoutingTable::new(),
            lsa_database: HashMap::new(),

            sequence_number: 1,

            socket,
            control_rx,
            control_tx,
        })
    }

    pub fn get_control_sender(&self) -> mpsc::Sender<ProtocolControl> {
        self.control_tx.clone()
    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
        info!("Starting routing protocol for router {}", self.router_id);

        // Initialize with connected routes
        self.initialize_connected_routes();

        // Start periodic tasks
        let hello_interval = Duration::from_secs(self.config.hello_interval as u64);
        let mut hello_timer = interval(hello_interval);

        let lsa_refresh_interval = Duration::from_secs(self.config.lsa_refresh_interval as u64);
        let mut lsa_timer = interval(lsa_refresh_interval);

        let neighbor_check_interval = Duration::from_secs(5);
        let mut neighbor_timer = interval(neighbor_check_interval);

        let mut buffer = vec![0u8; 65536];

        loop {
            tokio::select! {
                // Handle incoming messages
                result = self.socket.recv_from(&mut buffer) => {
                    match result {
                        Ok((len, src)) => {
                            if let Err(e) = self.handle_incoming_message(&buffer[..len], src).await {
                                error!("Error handling message from {}: {}", src, e);
                            }
                        }
                        Err(e) => error!("Error receiving message: {}", e),
                    }
                }

                // Send hello messages
                _ = hello_timer.tick() => {
                    if self.enabled {
                        if let Err(e) = self.send_hello_messages().await {
                            error!("Error sending hello messages: {}", e);
                        }
                    }
                }

                // Send LSA updates
                _ = lsa_timer.tick() => {
                    if self.enabled {
                        if let Err(e) = self.send_lsa_updates().await {
                            error!("Error sending LSA updates: {}", e);
                        }
                    }
                }

                // Check for dead neighbors
                _ = neighbor_timer.tick() => {
                    if self.enabled {
                        let dead_neighbors = self.neighbor_manager.check_dead_neighbors();
                        if !dead_neighbors.is_empty() {
                            info!("Detected dead neighbors: {:?}", dead_neighbors);
                            self.topology_changed().await;
                        }
                    }
                }

                // Handle control messages
                Some(control) = self.control_rx.recv() => {
                    self.handle_control_message(control).await;
                }
            }
        }
    }

    async fn handle_incoming_message(&mut self, data: &[u8], src: SocketAddr) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let message = ProtocolMessage::deserialize(data)?;
        debug!("Received message from {}: {:?}", src, message);

        match message {
            ProtocolMessage::Hello(hello) => {
                self.handle_hello_message(hello, src.ip()).await?;
            }
            ProtocolMessage::LinkStateAdvertisement(lsa) => {
                self.handle_lsa_message(lsa).await?;
            }
            _ => {
                debug!("Unhandled message type from {}", src);
            }
        }

        Ok(())
    }

    async fn handle_hello_message(&mut self, hello: HelloMessage, src_ip: IpAddr) -> anyhow::Result<()> {
        // Update or add neighbor
        if let Some(_) = self.neighbor_manager.get_neighbor(&hello.router_id) {
            self.neighbor_manager.update_neighbor_hello(&hello.router_id);
        } else {
            // Find interface for this neighbor
            let interface = self.find_interface_for_ip(src_ip).unwrap_or_else(|| "unknown".to_string());

            let neighbor = Neighbor::new(
                hello.router_id.clone(),
                hello.router_name.clone(),
                src_ip,
                interface,
                hello.hello_interval,
                hello.dead_interval,
            );

            info!("New neighbor discovered: {} ({})", hello.router_name, hello.router_id);
            self.neighbor_manager.add_neighbor(neighbor);
            self.topology_changed().await;
        }

        Ok(())
    }

    async fn handle_lsa_message(&mut self, lsa: LSAMessage) -> anyhow::Result<()> {
        let lsa_key = format!("{}:{}", lsa.advertising_router, lsa.link_state_id);

        // Check if this is a newer LSA
        if let Some(existing) = self.lsa_database.get(&lsa_key) {
            if lsa.sequence_number <= existing.sequence_number {
                return Ok(()); // Ignore older LSAs
            }
        }

        // Store the LSA
        self.lsa_database.insert(lsa_key, lsa.clone());

        // Update topology
        self.topology.update_from_lsa(&lsa);

        // Recalculate routes
        self.calculate_routes().await;

        // Flood LSA to neighbors (except sender)
        self.flood_lsa(lsa).await?;

        Ok(())
    }

    async fn send_hello_messages(&self) -> anyhow::Result<()> {
        for interface in self.config.get_enabled_interfaces() {
            let neighbors: Vec<String> = self.neighbor_manager
                .get_neighbors_for_interface(&interface.name)
                .iter()
                .map(|n| n.router_id.clone())
                .collect();

            let hello = HelloMessage::new(
                self.router_id.clone(),
                self.router_name.clone(),
                self.config.hello_interval,
                self.config.dead_interval,
                neighbors,
            );

            let message = ProtocolMessage::Hello(hello);
            let data = message.serialize()?;

            // Send to multicast address for this interface
            let multicast_addr = format!("224.0.0.5:{}", 9090);
            if let Ok(addr) = multicast_addr.parse::<SocketAddr>() {
                if let Err(e) = self.socket.send_to(&data, &addr).await {
                    warn!("Failed to send hello on interface {}: {}", interface.name, e);
                }
            }
        }

        Ok(())
    }

    async fn send_lsa_updates(&mut self) -> anyhow::Result<()> {
        // Generate router LSA
        let links = self.generate_router_links();

        let lsa = LSAMessage::new_router_lsa(
            self.router_id.clone(),
            self.sequence_number,
            links,
        );

        self.sequence_number += 1;

        // Store our own LSA
        let lsa_key = format!("{}:{}", lsa.advertising_router, lsa.link_state_id);
        self.lsa_database.insert(lsa_key, lsa.clone());

        // Flood to all neighbors
        self.flood_lsa(lsa).await?;

        Ok(())
    }

    fn generate_router_links(&self) -> Vec<LinkData> {
        let mut links = Vec::new();

        for interface in self.config.get_enabled_interfaces() {
            // Add point-to-point links to neighbors
            for neighbor in self.neighbor_manager.get_neighbors_for_interface(&interface.name) {
                if neighbor.is_active() {
                    links.push(LinkData {
                        link_id: neighbor.router_id.clone(),
                        link_data: neighbor.ip_address,
                        link_type: 1, // Point-to-point
                        metric: interface.cost,
                        bandwidth: interface.bandwidth * 1_000_000, // Convert to bps
                        available_bandwidth: interface.bandwidth * 1_000_000, // Simplified
                    });
                }
            }

            // Add stub networks
            if let Ok(network) = interface.network.parse::<ipnet::Ipv4Net>() {
                links.push(LinkData {
                    link_id: format!("network-{}", interface.name),
                    link_data: IpAddr::V4(network.network()),
                    link_type: 3, // Stub network
                    metric: interface.cost,
                    bandwidth: interface.bandwidth * 1_000_000,
                    available_bandwidth: interface.bandwidth * 1_000_000,
                });
            }
        }

        links
    }

    async fn flood_lsa(&self, lsa: LSAMessage) -> anyhow::Result<()> {
        let message = ProtocolMessage::LinkStateAdvertisement(lsa);
        let data = message.serialize()?;

        for neighbor in self.neighbor_manager.get_active_neighbors() {
            let addr = SocketAddr::new(neighbor.ip_address, 9090);
            if let Err(e) = self.socket.send_to(&data, &addr).await {
                warn!("Failed to send LSA to neighbor {}: {}", neighbor.router_id, e);
            }
        }

        Ok(())
    }

    async fn topology_changed(&mut self) {
        info!("Topology changed, recalculating routes");
        self.calculate_routes().await;
    }

    async fn calculate_routes(&mut self) {
        // Clear existing dynamic routes
        self.routing_table.clear_dynamic_routes();

        // Calculate shortest paths using Dijkstra's algorithm
        let paths = self.topology.calculate_shortest_paths(&self.router_id);

        // Add routes to routing table
        for (destination, path) in &paths {  // <- Add & here to borrow instead of move
            if let Some(next_hop_id) = &path.next_hop {
                if let Some(neighbor) = self.neighbor_manager.get_neighbor(next_hop_id) {
                    // Find the network for this destination
                    if let Some(node) = self.topology.nodes.get(destination) {  // <- Remove & here since destination is now &String
                        for link in &node.links {
                            if link.link_type == 3 { // Stub network
                                if let IpAddr::V4(network_addr) = link.link_data {
                                    if let Ok(network) = ipnet::Ipv4Net::new(network_addr, 24) {
                                        let entry = RoutingEntry::new_dynamic(
                                            network,
                                            neighbor.ip_address,
                                            neighbor.interface.clone(),
                                            path.cost,
                                        );
                                        self.routing_table.add_route(entry);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Add default route if we are not the default router
        if !self.is_default_router {
            if let Some(default_node) = self.topology.get_default_router() {
                let default_router_id = default_node.router_id.clone();
                if let Some(path) = paths.get(&default_router_id) {  // <- Now paths is still available
                    if let Some(next_hop_id) = &path.next_hop {
                        if let Some(neighbor) = self.neighbor_manager.get_neighbor(next_hop_id) {
                            let default_route = RoutingEntry::new_default(
                                neighbor.ip_address,
                                neighbor.interface.clone(),
                                path.cost,
                            );
                            self.routing_table.set_default_route(default_route);
                        }
                    }
                }
            }
        }

        // Update system routing table
        if let Err(e) = self.routing_table.update_system_routing_table() {
            error!("Failed to update system routing table: {}", e);
        }
    }

    fn initialize_connected_routes(&mut self) {
        for interface in self.config.get_enabled_interfaces() {
            if let Ok(network) = interface.network.parse::<ipnet::Ipv4Net>() {
                let entry = RoutingEntry::new_connected(network, interface.name.clone());
                self.routing_table.add_route(entry);
            }
        }
    }

    fn find_interface_for_ip(&self, ip: IpAddr) -> Option<String> {
        for interface in self.config.get_enabled_interfaces() {
            if let Ok(network) = interface.network.parse::<ipnet::Ipv4Net>() {
                if let IpAddr::V4(ipv4) = ip {
                    if network.contains(&ipv4) {
                        return Some(interface.name.clone());
                    }
                }
            }
        }
        None
    }

    async fn handle_control_message(&mut self, control: ProtocolControl) {
        match control {
            ProtocolControl::Enable => {
                info!("Enabling routing protocol");
                self.enabled = true;
            }
            ProtocolControl::Disable => {
                info!("Disabling routing protocol");
                self.enabled = false;
            }
            ProtocolControl::Recalculate => {
                info!("Forced route recalculation");
                self.calculate_routes().await;
            }
            ProtocolControl::InterfaceUp(interface) => {
                info!("Interface {} is up", interface);
                // Mark links as active
                self.topology_changed().await;
            }
            ProtocolControl::InterfaceDown(interface) => {
                info!("Interface {} is down", interface);
                // Mark links as inactive
                self.topology_changed().await;
            }
            ProtocolControl::SetDefaultRouter(is_default) => {
                info!("Setting default router status: {}", is_default);
                self.is_default_router = is_default;
                if is_default {
                    self.topology.set_default_router(&self.router_id);
                }
                self.calculate_routes().await;
            }
        }
    }
}