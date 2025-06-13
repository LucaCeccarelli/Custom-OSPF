use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, error, warn};
use serde_json;

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
        let control_task = tokio::spawn(async move {
            Self::run_control_server(control_listener, protocol_control).await;
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
    ) {
        info!("Control server listening");

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    info!("Control connection from {}", addr);
                    let control_tx = protocol_control.clone();

                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_control_connection(stream, control_tx).await {
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
        control_tx: tokio::sync::mpsc::Sender<ProtocolControl>,
    ) -> anyhow::Result<()> {
        let mut buffer = vec![0; 1024];
        let n = stream.read(&mut buffer).await?;
        let command = String::from_utf8_lossy(&buffer[..n]).trim().to_string();

        info!("Received control command: {}", command);

        let response = match command.as_str() {
            "ENABLE" => {
                control_tx.send(ProtocolControl::Enable).await?;
                "Routing protocol enabled\n".to_string()
            }
            "DISABLE" => {
                control_tx.send(ProtocolControl::Disable).await?;
                "Routing protocol disabled\n".to_string()
            }
            "NEIGHBORS" => {
                // This would require querying the protocol state
                "Neighbor list command received\n".to_string()
            }
            "ROUTES" => {
                // This would require querying the routing table
                "Routing table command received\n".to_string()
            }
            "TOPOLOGY" => {
                // This would require querying the topology
                "Network topology command received\n".to_string()
            }
            "RECALCULATE" => {
                control_tx.send(ProtocolControl::Recalculate).await?;
                "Route recalculation triggered\n".to_string()
            }
            _ => {
                format!("Unknown command: {}\n", command)
            }
        };

        stream.write_all(response.as_bytes()).await?;
        Ok(())
    }
}