mod neighbor_manager;
mod route_manager;
mod task_manager;
mod types;

pub use types::*;
use crate::network::Network;
use crate::router::Router;
use crate::routing_table::{RouteEntry, RouteSource};
use log::{info, error};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, oneshot, broadcast};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct SimpleRoutingProtocol {
    router: Arc<Mutex<Router>>,
    sockets: Vec<Arc<UdpSocket>>,
    neighbors: Arc<Mutex<HashMap<String, NeighborInfo>>>,
    route_states: Arc<Mutex<HashMap<String, RouteState>>>,
    sequence: Arc<Mutex<u64>>,
    port: u16,
    debug_mode: bool,
    our_ips: HashSet<Ipv4Addr>,
    is_running: Arc<AtomicBool>,
    shutdown_tx: Arc<Mutex<Option<broadcast::Sender<()>>>>,
    task_handles: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    pending_neighbor_requests: Arc<Mutex<HashMap<String, PendingNeighborRequest>>>,
}

impl SimpleRoutingProtocol {
    // Constants for timeouts
    pub const NEIGHBOR_TIMEOUT: Duration = Duration::from_secs(12);
    pub const ROUTE_TIMEOUT: Duration = Duration::from_secs(16);
    pub const HELLO_INTERVAL: Duration = Duration::from_secs(4);
    pub const UPDATE_INTERVAL: Duration = Duration::from_secs(8);
    pub const CLEANUP_INTERVAL: Duration = Duration::from_secs(5);

    pub async fn new(
        router_id: String,
        interface_names: HashSet<String>,
        port: u16,
        network: Network,
        debug_mode: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let router = Router::new(
            router_id,
            network,
            interface_names.into_iter().collect(),
        ).await?;

        let mut sockets = Vec::new();
        let mut our_ips = HashSet::new();

        let bind_addr = format!("0.0.0.0:{}", port);
        match UdpSocket::bind(&bind_addr).await {
            Ok(socket) => {
                socket.set_broadcast(true)?;
                info!("✓ Socket bound to {} for all interfaces", bind_addr);
                sockets.push(Arc::new(socket));

                for (_, interface) in &router.interfaces {
                    our_ips.insert(interface.ip);
                }
            }
            Err(e) => {
                return Err(format!("Failed to bind socket: {}", e).into());
            }
        }

        if sockets.is_empty() {
            return Err("No sockets could be created".into());
        }

        info!("Router created successfully");
        info!("Bound to {} sockets on port {}", sockets.len(), port);
        info!("Our IPs: {:?}", our_ips);

        Ok(Self {
            router: Arc::new(Mutex::new(router)),
            sockets,
            neighbors: Arc::new(Mutex::new(HashMap::new())),
            route_states: Arc::new(Mutex::new(HashMap::new())),
            sequence: Arc::new(Mutex::new(0)),
            port,
            debug_mode,
            our_ips,
            is_running: Arc::new(AtomicBool::new(false)),
            shutdown_tx: Arc::new(Mutex::new(None)),
            task_handles: Arc::new(Mutex::new(Vec::new())),
            pending_neighbor_requests: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    // Main control methods
    pub async fn start_protocol(&self) -> Result<(), Box<dyn std::error::Error>> {
        if self.is_running.load(Ordering::Relaxed) {
            return Ok(());
        }

        info!("Starting routing protocol...");
        self.is_running.store(true, Ordering::Relaxed);

        let (shutdown_tx, _) = broadcast::channel(1);
        {
            let mut tx_guard = self.shutdown_tx.lock().await;
            *tx_guard = Some(shutdown_tx);
        }

        task_manager::start_tasks(self).await?;
        info!("✓ Routing protocol started successfully");
        Ok(())
    }

    pub async fn stop_protocol(&self) -> Result<(), Box<dyn std::error::Error>> {
        if !self.is_running.load(Ordering::Relaxed) {
            return Ok(());
        }

        info!("Stopping routing protocol...");
        self.is_running.store(false, Ordering::Relaxed);

        {
            let tx_guard = self.shutdown_tx.lock().await;
            if let Some(ref tx) = *tx_guard {
                let _ = tx.send(());
            }
        }

        {
            let mut handles_guard = self.task_handles.lock().await;
            for handle in handles_guard.drain(..) {
                handle.abort();
            }
        }

        info!("✓ Routing protocol stopped successfully");
        Ok(())
    }

    // Status and information methods
    pub async fn get_status(&self) -> (String, bool) {
        let router_guard = self.router.lock().await;
        let router_id = router_guard.id.clone();
        drop(router_guard);

        let is_running = self.is_running.load(Ordering::Relaxed);
        (router_id, is_running)
    }

    pub async fn get_neighbors(&self) -> Vec<crate::control_server::NeighborInfo> {
        neighbor_manager::get_neighbors_list(&self.neighbors).await
    }

    pub async fn get_neighbors_of(&self, router_id: &str) -> Option<Vec<crate::control_server::NeighborInfo>> {
        neighbor_manager::get_neighbors_of(self, router_id).await
    }

    pub async fn get_routing_table(&self) -> Vec<serde_json::Value> {
        let router_guard = self.router.lock().await;
        let routes = router_guard.routing_table.get_routes();

        routes.iter().map(|route| {
            serde_json::json!({
                "destination": route.destination.to_string(),
                "next_hop": if route.next_hop.is_unspecified() {
                    "direct".to_string()
                } else {
                    route.next_hop.to_string()
                },
                "interface": route.interface,
                "metric": route.metric,
                "source": format!("{:?}", route.source)
            })
        }).collect()
    }
    
    // Internal accessor methods for other modules
    pub fn get_router(&self) -> &Arc<Mutex<Router>> {
        &self.router
    }

    pub fn get_sockets(&self) -> &[Arc<UdpSocket>] {
        &self.sockets
    }

    pub fn get_neighbors_arc(&self) -> &Arc<Mutex<HashMap<String, NeighborInfo>>> {
        &self.neighbors
    }

    pub fn get_route_states(&self) -> &Arc<Mutex<HashMap<String, RouteState>>> {
        &self.route_states
    }

    pub fn get_sequence(&self) -> &Arc<Mutex<u64>> {
        &self.sequence
    }

    pub fn get_port(&self) -> u16 {
        self.port
    }

    pub fn is_debug_mode(&self) -> bool {
        self.debug_mode
    }

    pub fn get_our_ips(&self) -> &HashSet<Ipv4Addr> {
        &self.our_ips
    }

    pub fn is_running(&self) -> &Arc<AtomicBool> {
        &self.is_running
    }

    pub fn get_shutdown_tx(&self) -> &Arc<Mutex<Option<broadcast::Sender<()>>>> {
        &self.shutdown_tx
    }

    pub fn get_task_handles(&self) -> &Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>> {
        &self.task_handles
    }

    pub fn get_pending_neighbor_requests(&self) -> &Arc<Mutex<HashMap<String, PendingNeighborRequest>>> {
        &self.pending_neighbor_requests
    }
}