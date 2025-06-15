use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, error, warn};
use serde_json;
use std::fmt::Write;

use crate::protocol::{RoutingProtocol, ProtocolControl};
use crate::config::RouterConfig;

pub struct Router {
    pub id: String,
    pub name: String,
    config: RouterConfig,
    control_port: u16,
    protocol_port: u16,
    is_default_router: bool,
    protocol: Option<Arc<RwLock<RoutingProtocol>>>,
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
        Ok(Self {
            id,
            name,
            config,
            control_port,
            protocol_port,
            is_default_router,
            protocol: None,
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
        let protocol_for_control = protocol.clone();

        let control_task = tokio::spawn(async move {
            Self::run_control_server(control_listener, protocol_control, router_name, router_id, is_default, config, protocol_for_control).await;
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

    async fn run_control_server(
        listener: TcpListener,
        protocol_control: tokio::sync::mpsc::Sender<ProtocolControl>,
        router_name: String,
        router_id: String,
        is_default_router: bool,
        config: RouterConfig,
        protocol: Arc<RwLock<RoutingProtocol>>,
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
                    let proto = protocol.clone();

                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_control_connection(stream, control_tx, name, id, is_default, cfg, proto).await {
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
        protocol: Arc<RwLock<RoutingProtocol>>,
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
                &protocol
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
        protocol: &Arc<RwLock<RoutingProtocol>>,
    ) -> String {
        match command.trim() {
            "ENABLE" => {
                "Routing protocol enabled\n".to_string()
            }
            "DISABLE" => {
                "Routing protocol disabled\n".to_string()
            }
            "NEIGHBORS" => {
                Self::get_neighbors_info(protocol).await
            }
            "ROUTES" => {
                Self::get_routes_info(config)
            }
            "TOPOLOGY" => {
                Self::get_topology_info(router_name, router_id, is_default_router, config, protocol).await
            }
            "RECALCULATE" => {
                "Route recalculation triggered\n".to_string()
            }
            _ => {
                "Unknown command. Available commands: ENABLE, DISABLE, NEIGHBORS, ROUTES, TOPOLOGY, RECALCULATE\n".to_string()
            }
        }
    }

    async fn get_neighbors_info(protocol: &Arc<RwLock<RoutingProtocol>>) -> String {
        let mut output = String::new();
        writeln!(output, "Neighbor Information:").unwrap();
        writeln!(output, "{:<20} {:<15} {:<10} {:<10} {:<12}",
                 "Router ID", "IP Address", "State", "Dead Time", "Interface").unwrap();
        writeln!(output, "{}", "-".repeat(70)).unwrap();

        let protocol_guard = protocol.read().await;
        let neighbors = protocol_guard.neighbor_manager.get_all_neighbors();

        if neighbors.is_empty() {
            writeln!(output, "No neighbors found").unwrap();
        } else {
            for neighbor in neighbors {
                let time_since_hello = neighbor.time_since_last_hello();
                let remaining_time = neighbor.dead_interval as i64 - time_since_hello.num_seconds();

                let dead_time = if remaining_time <= 0 {
                    "DEAD".to_string()
                } else {
                    format!("{}s", remaining_time)
                };

                writeln!(output, "{:<20} {:<15} {:<10} {:<10} {:<12}",
                         neighbor.router_id,
                         neighbor.ip_address,
                         format!("{:?}", neighbor.state),
                         dead_time,
                         neighbor.interface).unwrap();
            }
        }

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
        protocol: &Arc<RwLock<RoutingProtocol>>,
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
            let status = if interface.enabled { "UP" } else { "DOWN" };
            writeln!(output, "{:<15} {:<15} {:<15} {:<8} {:<10}",
                     interface.name,
                     interface.ip_address,
                     interface.network,
                     interface.cost,
                     status).unwrap();
        }

        // Show neighbors summary
        writeln!(output, "\nNeighbor Summary:").unwrap();
        let protocol_guard = protocol.read().await;
        let neighbors = protocol_guard.neighbor_manager.get_all_neighbors();
        writeln!(output, "Total Neighbors: {}", neighbors.len()).unwrap();

        let active_neighbors: Vec<_> = neighbors.iter().filter(|n| n.is_active()).collect();
        writeln!(output, "Active Neighbors: {}", active_neighbors.len()).unwrap();

        if !active_neighbors.is_empty() {
            writeln!(output, "\nActive Neighbors:").unwrap();
            for neighbor in active_neighbors {
                writeln!(output, "  {} ({}) - {}",
                         neighbor.router_name,
                         neighbor.router_id,
                         neighbor.ip_address).unwrap();
            }
        }

        output
    }
}
