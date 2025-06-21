use crate::network::Network;
use crate::router::{Router, RouterInfo};
use crate::routing_table::{RouteEntry, RouteSource};
use log::{info, warn, error};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::time::interval;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RoutingUpdate {
    pub router_id: String,
    pub sequence: u64,
    pub routes: Vec<RouteInfo>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RouteInfo {
    pub destination: String, // Network as string
    pub metric: u32,
    pub next_hop: String, // IP as string
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HelloMessage {
    pub router_id: String,
    pub interfaces: HashMap<String, String>, // interface -> IP
}

pub struct SimpleRoutingProtocol {
    router: Arc<Mutex<Router>>,
    socket: UdpSocket,
    neighbors: Arc<Mutex<HashMap<String, (SocketAddr, RouterInfo)>>>,
    sequence: Arc<Mutex<u64>>,
    multicast_addr: SocketAddr,
}

impl SimpleRoutingProtocol {
    pub async fn new(
        router_id: String,
        interface_names: HashSet<String>,
        port: u16,
        network: Network,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", port)).await?;
        socket.set_broadcast(true)?;

        let router = Router::new(
            router_id,
            network,
            interface_names.into_iter().collect(),
        )?;

        Ok(Self {
            router: Arc::new(Mutex::new(router)),
            socket,
            neighbors: Arc::new(Mutex::new(HashMap::new())),
            sequence: Arc::new(Mutex::new(0)),
            multicast_addr: format!("255.255.255.255:{}", port).parse()?,
        })
    }

    pub async fn start(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        info!("Starting routing protocol...");

        // Démarrer les tâches
        let hello_task = self.start_hello_task();
        let update_task = self.start_update_task();
        let listen_task = self.start_listen_task();

        // Attendre toutes les tâches
        tokio::try_join!(hello_task, update_task, listen_task)?;

        Ok(())
    }

    async fn start_hello_task(&self) -> Result<(), Box<dyn std::error::Error>> {
        let socket = &self.socket;
        let router = self.router.clone();
        let multicast_addr = self.multicast_addr;

        let mut interval = interval(Duration::from_secs(30));

        loop {
            interval.tick().await;

            let router_guard = router.lock().await;
            let router_info = router_guard.get_router_info();
            drop(router_guard);

            let hello = HelloMessage {
                router_id: router_info.router_id,
                interfaces: router_info.interfaces
                    .iter()
                    .map(|(name, info)| (name.clone(), info.ip.to_string()))
                    .collect(),
            };

            let message = serde_json::to_string(&hello)?;
            let hello_packet = format!("HELLO:{}", message);

            if let Err(e) = socket.send_to(hello_packet.as_bytes(), multicast_addr).await {
                warn!("Failed to send hello: {}", e);
            } else {
                info!("Sent hello message");
            }
        }
    }

    async fn start_update_task(&self) -> Result<(), Box<dyn std::error::Error>> {
        let socket = &self.socket;
        let router = self.router.clone();
        let sequence = self.sequence.clone();
        let multicast_addr = self.multicast_addr;

        let mut interval = interval(Duration::from_secs(60));

        loop {
            interval.tick().await;

            let mut seq_guard = sequence.lock().await;
            *seq_guard += 1;
            let current_seq = *seq_guard;
            drop(seq_guard);

            let router_guard = router.lock().await;
            let routes: Vec<RouteInfo> = router_guard
                .routing_table
                .get_routes()
                .iter()
                .map(|route| RouteInfo {
                    destination: route.destination.to_string(),
                    metric: route.metric,
                    next_hop: route.next_hop.to_string(),
                })
                .collect();
            let router_id = router_guard.id.clone();
            drop(router_guard);

            let update = RoutingUpdate {
                router_id,
                sequence: current_seq,
                routes,
            };

            let message = serde_json::to_string(&update)?;
            let update_packet = format!("UPDATE:{}", message);

            if let Err(e) = socket.send_to(update_packet.as_bytes(), multicast_addr).await {
                warn!("Failed to send routing update: {}", e);
            } else {
                info!("Sent routing update with {} routes", update.routes.len());
            }
        }
    }

    async fn start_listen_task(&self) -> Result<(), Box<dyn std::error::Error>> {
        let socket = &self.socket;
        let router = self.router.clone();
        let neighbors = self.neighbors.clone();

        let mut buffer = [0u8; 4096];

        loop {
            match socket.recv_from(&mut buffer).await {
                Ok((len, addr)) => {
                    let data = String::from_utf8_lossy(&buffer[..len]);

                    if let Err(e) = self.handle_message(&data, addr, &router, &neighbors).await {
                        warn!("Failed to handle message from {}: {}", addr, e);
                    }
                }
                Err(e) => {
                    error!("Failed to receive message: {}", e);
                }
            }
        }
    }

    async fn handle_message(
        &self,
        data: &str,
        addr: SocketAddr,
        router: &Arc<Mutex<Router>>,
        neighbors: &Arc<Mutex<HashMap<String, (SocketAddr, RouterInfo)>>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(hello_data) = data.strip_prefix("HELLO:") {
            let hello: HelloMessage = serde_json::from_str(hello_data)?;
            info!("Received hello from {}", hello.router_id);

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

            let mut neighbors_guard = neighbors.lock().await;
            neighbors_guard.insert(hello.router_id, (addr, router_info));

        } else if let Some(update_data) = data.strip_prefix("UPDATE:") {
            let update: RoutingUpdate = serde_json::from_str(update_data)?;
            info!("Received routing update from {} with {} routes",
                  update.router_id, update.routes.len());

            self.process_routing_update(update, router).await?;
        }

        Ok(())
    }

    async fn process_routing_update(
        &self,
        update: RoutingUpdate,
        router: &Arc<Mutex<Router>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut new_routes = Vec::new();

        for route_info in update.routes {
            if let (Ok(destination), Ok(next_hop)) = (
                route_info.destination.parse(),
                route_info.next_hop.parse(),
            ) {
                // Choisir une interface appropriée (simplification)
                let router_guard = router.lock().await;
                if let Some((_, interface)) = router_guard.interfaces.iter().next() {
                    let route = RouteEntry {
                        destination,
                        next_hop,
                        interface: interface.name.clone(),
                        metric: route_info.metric + 1, // +1 hop
                        source: RouteSource::Protocol,
                    };
                    new_routes.push(route);
                }
                drop(router_guard);
            }
        }

        // Mettre à jour la table de routage
        let mut router_guard = router.lock().await;
        router_guard.update_routing_table(new_routes)?;
        drop(router_guard);

        Ok(())
    }
}
