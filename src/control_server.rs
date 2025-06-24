use crate::protocol::SimpleRoutingProtocol;
use log::{info, warn, error};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

#[derive(Debug, Serialize, Deserialize)]
pub struct NeighborInfo {
    pub router_id: String,
    pub ip_address: String,
    pub last_seen: String,
    pub is_alive: bool,
    pub interfaces: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ControlResponse {
    pub success: bool,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ControlCommand {
    pub command: String,
    pub args: Option<serde_json::Value>,
}

pub struct ControlServer {
    port: u16,
    pub protocol: Arc<Mutex<Option<SimpleRoutingProtocol>>>,
}

impl ControlServer {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            protocol: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn set_protocol(&self, protocol: SimpleRoutingProtocol) {
        let mut protocol_guard = self.protocol.lock().await;
        *protocol_guard = Some(protocol);
    }

    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        let bind_addr = format!("127.0.0.1:{}", self.port);
        let listener = TcpListener::bind(&bind_addr).await?;
        info!("Control server listening on {}", bind_addr);

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    info!("Control connection from {}", addr);
                    let protocol_clone = self.protocol.clone();
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_client(stream, protocol_clone).await {
                            error!("Error handling control client {}: {}", addr, e);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to accept control connection: {}", e);
                }
            }
        }
    }

    async fn handle_client(
        mut stream: TcpStream,
        protocol: Arc<Mutex<Option<SimpleRoutingProtocol>>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (reader, mut writer) = stream.split();
        let mut buf_reader = BufReader::new(reader);
        let mut line = String::new();

        loop {
            line.clear();
            match buf_reader.read_line(&mut line).await {
                Ok(0) => break, // Connection closed
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    let response = match serde_json::from_str::<ControlCommand>(trimmed) {
                        Ok(command) => Self::process_command(command, &protocol).await,
                        Err(e) => ControlResponse {
                            success: false,
                            message: format!("Invalid JSON command: {}", e),
                            data: None,
                        },
                    };

                    let response_json = serde_json::to_string(&response)?;
                    writer.write_all(format!("{}\n", response_json).as_bytes()).await?;
                    writer.flush().await?;
                }
                Err(e) => {
                    error!("Error reading from control client: {}", e);
                    break;
                }
            }
        }

        Ok(())
    }

    async fn process_command(
        command: ControlCommand,
        protocol: &Arc<Mutex<Option<SimpleRoutingProtocol>>>,
    ) -> ControlResponse {
        match command.command.as_str() {
            "status" => Self::get_status(protocol).await,
            "neighbors" => Self::get_neighbors(protocol).await,
            "neighbors_of" => Self::get_neighbors_of(command.args, protocol).await,
            "start" => Self::start_protocol(protocol).await,
            "stop" => Self::stop_protocol(protocol).await,
            "routing_table" => Self::get_routing_table(protocol).await,
            "help" => Self::get_help(),
            _ => ControlResponse {
                success: false,
                message: format!("Unknown command: {}", command.command),
                data: None,
            },
        }
    }

    async fn get_status(
        protocol: &Arc<Mutex<Option<SimpleRoutingProtocol>>>,
    ) -> ControlResponse {
        let protocol_guard = protocol.lock().await;
        if let Some(ref prot) = *protocol_guard {
            let (router_id, is_running) = prot.get_status().await;
            ControlResponse {
                success: true,
                message: "Status retrieved".to_string(),
                data: Some(serde_json::json!({
                    "router_id": router_id,
                    "is_running": is_running,
                    "uptime": "N/A" // Could add uptime tracking
                })),
            }
        } else {
            ControlResponse {
                success: false,
                message: "Protocol not initialized".to_string(),
                data: None,
            }
        }
    }

    async fn get_neighbors(
        protocol: &Arc<Mutex<Option<SimpleRoutingProtocol>>>,
    ) -> ControlResponse {
        let protocol_guard = protocol.lock().await;
        if let Some(ref prot) = *protocol_guard {
            let neighbors = prot.get_neighbors().await;
            ControlResponse {
                success: true,
                message: format!("Found {} neighbors", neighbors.len()),
                data: Some(serde_json::to_value(neighbors).unwrap()),
            }
        } else {
            ControlResponse {
                success: false,
                message: "Protocol not initialized".to_string(),
                data: None,
            }
        }
    }

    async fn get_neighbors_of(
        args: Option<serde_json::Value>,
        protocol: &Arc<Mutex<Option<SimpleRoutingProtocol>>>,
    ) -> ControlResponse {
        let router_id = match args {
            Some(serde_json::Value::String(id)) => id,
            Some(obj) if obj.is_object() => {
                match obj.get("router_id") {
                    Some(serde_json::Value::String(id)) => id.clone(),
                    _ => return ControlResponse {
                        success: false,
                        message: "Missing or invalid router_id parameter".to_string(),
                        data: None,
                    }
                }
            }
            _ => return ControlResponse {
                success: false,
                message: "router_id parameter required".to_string(),
                data: None,
            }
        };

        let protocol_guard = protocol.lock().await;
        if let Some(ref prot) = *protocol_guard {
            match prot.get_neighbors_of(&router_id).await {
                Some(neighbors) => ControlResponse {
                    success: true,
                    message: format!("Found {} neighbors for router {}", neighbors.len(), router_id),
                    data: Some(serde_json::to_value(neighbors).unwrap()),
                },
                None => ControlResponse {
                    success: false,
                    message: format!("Router {} not found or no neighbor information available", router_id),
                    data: None,
                }
            }
        } else {
            ControlResponse {
                success: false,
                message: "Protocol not initialized".to_string(),
                data: None,
            }
        }
    }

    async fn start_protocol(
        protocol: &Arc<Mutex<Option<SimpleRoutingProtocol>>>,
    ) -> ControlResponse {
        let protocol_guard = protocol.lock().await;
        if let Some(ref prot) = *protocol_guard {
            match prot.start_protocol().await {
                Ok(_) => ControlResponse {
                    success: true,
                    message: "Protocol started successfully".to_string(),
                    data: None,
                },
                Err(e) => ControlResponse {
                    success: false,
                    message: format!("Failed to start protocol: {}", e),
                    data: None,
                }
            }
        } else {
            ControlResponse {
                success: false,
                message: "Protocol not initialized".to_string(),
                data: None,
            }
        }
    }

    async fn stop_protocol(
        protocol: &Arc<Mutex<Option<SimpleRoutingProtocol>>>,
    ) -> ControlResponse {
        let protocol_guard = protocol.lock().await;
        if let Some(ref prot) = *protocol_guard {
            match prot.stop_protocol().await {
                Ok(_) => ControlResponse {
                    success: true,
                    message: "Protocol stopped successfully".to_string(),
                    data: None,
                },
                Err(e) => ControlResponse {
                    success: false,
                    message: format!("Failed to stop protocol: {}", e),
                    data: None,
                }
            }
        } else {
            ControlResponse {
                success: false,
                message: "Protocol not initialized".to_string(),
                data: None,
            }
        }
    }

    async fn get_routing_table(
        protocol: &Arc<Mutex<Option<SimpleRoutingProtocol>>>,
    ) -> ControlResponse {
        let protocol_guard = protocol.lock().await;
        if let Some(ref prot) = *protocol_guard {
            let routes = prot.get_routing_table().await;
            ControlResponse {
                success: true,
                message: format!("Retrieved {} routes", routes.len()),
                data: Some(serde_json::to_value(routes).unwrap()),
            }
        } else {
            ControlResponse {
                success: false,
                message: "Protocol not initialized".to_string(),
                data: None,
            }
        }
    }

    fn get_help() -> ControlResponse {
        let commands = vec![
            ("status", "Get protocol status"),
            ("neighbors", "Get list of current neighbors"),
            ("neighbors_of", "Get neighbors of specific router (requires router_id)"),
            ("start", "Start the routing protocol"),
            ("stop", "Stop the routing protocol"),
            ("routing_table", "Get current routing table"),
            ("help", "Show this help message"),
        ];

        ControlResponse {
            success: true,
            message: "Available commands".to_string(),
            data: Some(serde_json::to_value(commands).unwrap()),
        }
    }
}