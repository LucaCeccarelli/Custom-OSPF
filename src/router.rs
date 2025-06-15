use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, error, warn};
use serde_json;
use std::fmt::Write;
use std::collections::HashMap;
use std::time::Duration;

use crate::protocol::{RoutingProtocol, ProtocolControl};
use crate::config::RouterConfig;
use crate::neighbor::{Neighbor, NeighborState};

#[derive(Debug, Clone)]
pub struct RouterStatus {
    pub neighbors: HashMap<String, Neighbor>,
    pub last_update: std::time::Instant,
}

pub struct Router {
    pub id: String,
    pub name: String,
    config: RouterConfig,
    control_port: u16,
    protocol_port: u16,
    is_default_router: bool,
    protocol: Option<Arc<RwLock<RoutingProtocol>>>,
    status: Arc<RwLock<RouterStatus>>,
}

impl Router {
    pub async fn new(
        id: String,
        name: String,
        config: RouterConfig,
        control_port: u16,
        protocol_port: u16,
        is_default_router: bool,
    ) -> anyhow::Result<Self> {
        let status = RouterStatus {
            neighbors: HashMap::new(),
            last_update: std::time::Instant::now(),
        };

        Ok(Self {
            id,
            name,
            config,
            control_port,
            protocol_port,
            is_default_router,
            protocol: None,
            status: Arc::new(RwLock::new(status)),
        })
    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
        info!("Starting router {} on ports {} (control) and {} (protocol)",
               self.name, self.control_port, self.protocol_port);

        // Create routing protocol
        let protocol_addr = SocketAddr::from(([0, 0, 0, 0], self.protocol_port));
        let protocol = RoutingProtocol::new(
            self.id.clone(),
            self.name.clone(),
            self.config.clone(),
            protocol_addr,
            self.is_default_router,
        ).await?;

        let protocol_control = protocol.get_control_sender();
        let protocol = Arc::new(RwLock::new(protocol));
        self.protocol = Some(protocol.clone());

        // Start control server
        let control_addr = SocketAddr::from(([0, 0, 0, 0], self.control_port));
        let control_listener = TcpListener::bind(control_addr).await?;

        // Start status updater
        let status_clone = self.status.clone();
        let protocol_for_status = protocol.clone();
        tokio::spawn(async move {
            Self::status_updater(status_clone, protocol_for_status).await;
        });

        // Start protocol in background
        let protocol_clone = protocol.clone();
        let protocol_task = tokio::spawn(async move {
            let mut protocol = protocol_clone.write().await;
            if let Err(e) = protocol.start().await {
                error!("Protocol error: {}", e);
            }
        });

        // Start control server in background
        let router_name = self.name.clone();
        let router_id = self.id.clone();
        let is_default = self.is_default_router;
        let config = self.config.clone();
        let status_for_control = self.status.clone();

        let control_task = tokio::spawn(async move {
            Self::run_control_server(control_listener, protocol_control, router_name, router_id, is_default, config, status_for_control).await;
        });

        // Wait for either task to complete
        tokio::select! {
            _ = protocol_task => {
                error!("Protocol task terminated");
            }
            _ = control_task => {
                error!("Control server task terminated");
            }
        }

        Ok(())
    }

    async fn status_updater(
        status: Arc<RwLock<RouterStatus>>,
        protocol: Arc<RwLock<RoutingProtocol>>,
    ) {
        let mut interval = tokio::time::interval(Duration::from_secs(1));

        loop {
            interval.tick().await;

            // Try to update status without blocking
            if let Ok(protocol_guard) = protocol.try_read() {
                let neighbors = protocol_guard.neighbor_manager.get_neighbors().clone();

                if let Ok(mut status_guard) = status.try_write() {
                    status_guard.neighbors = neighbors;
                    status_guard.last_update = std::time::Instant::now();
                }
            }
        }
    }

    async fn run_control_server(
        listener: TcpListener,
        protocol_control: tokio::sync::mpsc::Sender<ProtocolControl>,
        router_name: String,
        router_id: String,
        is_default_router: bool,
        config: RouterConfig,
        status: Arc<RwLock<RouterStatus>>,
    ) {
        info!("Control server listening");

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    info!("Control connection from {}", addr);
                    let control_tx = protocol_control.clone();
                    let name = router_name.clone();
                    let id = router_id.clone();
                    let is_default = is_default_router;
                    let cfg = config.clone();
                    let status_clone = status.clone();

                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_control_connection(stream, control_tx, name, id, is_default, cfg, status_clone).await {
                            error!("Control connection error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to accept control connection: {}", e);
                }
            }
        }
    }

    async fn handle_control_connection(
        mut stream: TcpStream,
        _protocol_control: tokio::sync::mpsc::Sender<ProtocolControl>,
        router_name: String,
        router_id: String,
        is_default_router: bool,
        config: RouterConfig,
        status: Arc<RwLock<RouterStatus>>,
    ) -> anyhow::Result<()> {
        let mut buffer = [0; 1024];

        loop {
            let bytes_read = stream.read(&mut buffer).await?;
            if bytes_read == 0 {
                break;
            }

            let command = String::from_utf8_lossy(&buffer[..bytes_read]);
            let response = Self::handle_control_command(
                command.trim(),
                &router_name,
                &router_id,
                is_default_router,
                &config,
                &status
            ).await;

            stream.write_all(response.as_bytes()).await?;
        }

        Ok(())
    }

    async fn handle_control_command(
        command: &str,
        router_name: &str,
        router_id: &str,
        is_default_router: bool,
        config: &RouterConfig,
        status: &Arc<RwLock<RouterStatus>>,
    ) -> String {
        match command.trim() {
            "ENABLE" => {
                "Routing protocol enabled\n".to_string()
            }
            "DISABLE" => {
                "Routing protocol disabled\n".to_string()
            }
            "NEIGHBORS" => {
                Self::get_neighbors_info(status).await
            }
            "ROUTES" => {
                Self::get_routes_info(config)
            }
            "TOPOLOGY" => {
                Self::get_topology_info(router_name, router_id, is_default_router, config, status).await
            }
            "RECALCULATE" => {
                "Route recalculation triggered\n".to_string()
            }
            _ => {
                "Unknown command. Available commands: ENABLE, DISABLE, NEIGHBORS, ROUTES, TOPOLOGY, RECALCULATE\n".to_string()
            }
        }
    }

    async fn get_neighbors_info(status: &Arc<RwLock<RouterStatus>>) -> String {
        let mut output = String::new();
        writeln!(output, "Neighbor Information:").unwrap();
        writeln!(output, "{:<20} {:<15} {:<10} {:<12} {:<12}",
                 "Router ID", "IP Address", "State", "Last Hello", "Interface").unwrap();
        writeln!(output, "{}", "-".repeat(75)).unwrap();

        let status_guard = status.read().await;            
        let neighbors = &status_guard.neighbors;
        if neighbors.is_empty() {
                writeln!(output, "No neighbors found").unwrap();
        } else {
                for neighbor in neighbors.values() {
                    let time_since_hello = neighbor.time_since_last_hello();
                    let last_hello = if time_since_hello.num_seconds() < 60 {
                        format!("{}s ago", time_since_hello.num_seconds())
                    } else {
                        let minutes = time_since_hello.num_minutes();
                        format!("{}m ago", minutes)
                    };

                    writeln!(output, "{:<20} {:<15} {:<10} {:<12} {:<12}",
                             neighbor.router_id,
                             neighbor.ip_address,
                             format!("{:?}", neighbor.state),
                             last_hello,
                             neighbor.interface).unwrap();
                }
            }

            let age = status_guard.last_update.elapsed().as_secs();
            writeln!(output, "\n(Data updated {} seconds ago)", age).unwrap();

        output
    }

    fn get_routes_info(config: &RouterConfig) -> String {
        let mut output = String::new();
        writeln!(output, "Routing Table:").unwrap();
        writeln!(output, "{:<18} {:<15} {:<12} {:<8}",
                 "Network", "Next Hop", "Interface", "Metric").unwrap();
        writeln!(output, "{}", "-".repeat(60)).unwrap();

        let interfaces = config.get_enabled_interfaces();
        if interfaces.is_empty() {
            writeln!(output, "No routes found").unwrap();
        } else {
            for interface in interfaces {
                writeln!(output, "{:<18} {:<15} {:<12} {:<8}",
                         interface.network,
                         "Direct",
                         interface.name,
                         interface.cost).unwrap();
            }
        }

        output
    }

    async fn get_topology_info(
        router_name: &str,
        router_id: &str,
        is_default_router: bool,
        config: &RouterConfig,
        status: &Arc<RwLock<RouterStatus>>,
    ) -> String {
        let mut output = String::new();
        writeln!(output, "Network Topology:").unwrap();
        writeln!(output, "{}", "=".repeat(50)).unwrap();

        // Show local router info
        writeln!(output, "\nLocal Router:").unwrap();
        writeln!(output, "Router ID: {}", router_id).unwrap();
        writeln!(output, "Router Name: {}", router_name).unwrap();
        writeln!(output, "Default Router: {}", if is_default_router { "YES" } else { "NO" }).unwrap();

        // Show interfaces
        writeln!(output, "\nInterfaces:").unwrap();
        writeln!(output, "{:<15} {:<15} {:<15} {:<8} {:<10}",
                 "Name", "IP Address", "Network", "Cost", "Status").unwrap();
        writeln!(output, "{}", "-".repeat(70)).unwrap();

        for interface in &config.interfaces {
            let status_str = if interface.enabled { "UP" } else { "DOWN" };
            writeln!(output, "{:<15} {:<15} {:<15} {:<8} {:<10}",
                     interface.name,
                     interface.ip_address,
                     interface.network,
                     interface.cost,
                     status_str).unwrap();
        }

        // Show neighbors summary
        writeln!(output, "\nNeighbor Summary:").unwrap();

        let status_guard = status.read().await;            let neighbors = &status_guard.neighbors;
        writeln!(output, "Total Neighbors: {}", neighbors.len()).unwrap();

        let active_neighbors: Vec<_> = neighbors.values().filter(|n| n.is_active()).collect();
        writeln!(output, "Active Neighbors: {}", active_neighbors.len()).unwrap();

        if !active_neighbors.is_empty() {
                writeln!(output, "\nActive Neighbors:").unwrap();
                for neighbor in active_neighbors {
                    writeln!(output, "  {} ({}) - {} [{}]",
                             neighbor.router_name,
                             neighbor.router_id,
                             neighbor.ip_address,
                             neighbor.interface).unwrap();
                }
            }

        let age = status_guard.last_update.elapsed().as_secs();
        writeln!(output, "\n(Neighbor data updated {} seconds ago)", age).unwrap();
        output
    }
}
