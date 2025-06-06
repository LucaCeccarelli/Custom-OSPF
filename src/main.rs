mod types;
mod network;
mod routing;
mod system;

use clap::{Args, Parser, Subcommand};
use std::net::{IpAddr, SocketAddr};
use tokio::time::{interval, Duration};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Parser)]
#[command(name = "custom-ospf")]
#[command(about = "A custom routing protocol implementation")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the routing protocol daemon
    Start(StartArgs),
    /// Show neighbor list
    Neighbors,
    /// Show routing table
    Routes,
    /// Enable/disable protocol
    Control { enable: bool },
    /// Configure interfaces
    Interface {
        #[arg(long)]
        add: Option<String>,
        #[arg(long)]
        remove: Option<String>,
        #[arg(long)]
        list: bool,
    },
}

#[derive(Args)]
struct StartArgs {
    /// Local IP address to bind to
    #[arg(long, default_value = "0.0.0.0")]
    bind_ip: IpAddr,

    /// Port to bind to
    #[arg(long, default_value = "5000")]
    port: u16,

    /// Hostname for this router
    #[arg(long)]
    hostname: Option<String>,

    /// Update interval in seconds
    #[arg(long, default_value = "30")]
    update_interval: u64,
}

struct RoutingDaemon {
    network_manager: network::NetworkManager,
    routing_engine: Arc<Mutex<routing::RoutingEngine>>,
    enabled: bool,
}

impl RoutingDaemon {
    pub fn new(bind_addr: SocketAddr, hostname: String, local_ip: IpAddr) -> Result<Self, Box<dyn std::error::Error>> {
        let network_manager = network::NetworkManager::new(bind_addr, hostname)?;
        let routing_engine = Arc::new(Mutex::new(routing::RoutingEngine::new(local_ip)));

        Ok(RoutingDaemon {
            network_manager,
            routing_engine,
            enabled: true,
        })
    }

    pub async fn run(&mut self, update_interval: Duration) -> Result<(), Box<dyn std::error::Error>> {
        let mut update_timer = interval(update_interval);

        println!("Routing protocol started. Update interval: {:?}", update_interval);

        loop {
            tokio::select! {
                _ = update_timer.tick() => {
                    if self.enabled {
                        self.send_update().await?;
                    }
                }

                // Handle incoming messages
                result = self.network_manager.receive_routing_update() => {
                    match result {
                        Ok(message) => {
                            if self.enabled {
                                self.process_routing_message(message).await?;
                            }
                        }
                        Err(e) => {
                            eprintln!("Error receiving message: {}", e);
                        }
                    }
                }
            }
        }
    }

    async fn send_update(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let routing_engine = self.routing_engine.lock().await;
        let routes: Vec<types::Route> = routing_engine.get_neighbors()
            .into_iter()
            .map(|neighbor| types::Route {
                destination: ipnetwork::IpNetwork::new(neighbor.id, 32).unwrap(),
                next_hop: neighbor.id,
                metric: 1,
                hop_count: 1,
                path: vec![neighbor.id],
            })
            .collect();

        let neighbors = routing_engine.get_neighbors();
        drop(routing_engine);

        self.network_manager.send_routing_update(routes, neighbors).await?;
        Ok(())
    }

    async fn process_routing_message(&self, message: types::RoutingMessage) -> Result<(), Box<dyn std::error::Error>> {
        let mut routing_engine = self.routing_engine.lock().await;
        routing_engine.update_topology(message);
        let routes = routing_engine.compute_shortest_paths();
        drop(routing_engine);

        // Update system routing table
        system::SystemIntegration::update_routing_table(&routes)?;

        println!("Updated routing table with {} routes", routes.len());
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start(args) => {
            let bind_addr = SocketAddr::new(args.bind_ip, args.port);
            let hostname = args.hostname.unwrap_or_else(|| {
                std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string())
            });

            let mut daemon = RoutingDaemon::new(bind_addr, hostname, args.bind_ip)?;
            let update_interval = Duration::from_secs(args.update_interval);

            daemon.run(update_interval).await?;
        }

        Commands::Neighbors => {
            println!("Neighbor list functionality - would connect to running daemon");
            // Implementation would connect to running daemon via IPC
        }

        Commands::Routes => {
            println!("Route display functionality - would connect to running daemon");
            // Implementation would connect to running daemon via IPC
        }

        Commands::Control { enable } => {
            println!("Protocol {}", if enable { "enabled" } else { "disabled" });
            // Implementation would send control message to running daemon
        }

        Commands::Interface { add, remove, list } => {
            if list {
                match system::SystemIntegration::get_interface_list() {
                    Ok(interfaces) => {
                        println!("Available interfaces:");
                        for interface in interfaces {
                            println!("  {}", interface);
                        }
                    }
                    Err(e) => eprintln!("Error listing interfaces: {}", e),
                }
            }

            if let Some(interface) = add {
                println!("Would add interface: {}", interface);
            }

            if let Some(interface) = remove {
                println!("Would remove interface: {}", interface);
            }
        }
    }

    Ok(())
}
