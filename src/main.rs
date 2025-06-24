use clap::{Arg, Command};
use log::{info, error};
use std::collections::HashSet;
use std::sync::Arc;

mod control_server;
mod network;
mod protocol;
mod router;
mod routing_table;

use control_server::ControlServer;
use network::Network;
use protocol::SimpleRoutingProtocol;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Debug)
        .init();

    let matches = Command::new("Simple Routing Protocol")
        .version("1.0")
        .arg(
            Arg::new("interfaces")
                .long("interfaces")
                .value_name("INTERFACE")
                .help("Network interfaces to include in routing")
                .action(clap::ArgAction::Append)
                .required(true),
        )
        .arg(
            Arg::new("sysname")
                .long("sysname")
                .value_name("NAME")
                .help("System name for this router")
                .required(true),
        )
        .arg(
            Arg::new("listen-port")
                .long("listen-port")
                .value_name("PORT")
                .help("Port to listen for routing protocol messages")
                .default_value("5555"),
        )
        .arg(
            Arg::new("control-port")
                .long("control-port")
                .value_name("PORT")
                .help("Port for control server")
                .default_value("8080"),
        )
        .arg(
            Arg::new("debug")
                .long("debug")
                .help("Show routing table every 30 seconds")
                .action(clap::ArgAction::SetTrue),
        )
        .get_matches();

    let interfaces: Vec<String> = matches
        .get_many::<String>("interfaces")
        .unwrap()
        .cloned()
        .collect();

    let sysname = matches.get_one::<String>("sysname").unwrap().clone();
    let port: u16 = matches
        .get_one::<String>("listen-port")
        .unwrap()
        .parse()
        .expect("Invalid port number");

    let control_port: u16 = matches
        .get_one::<String>("control-port")
        .unwrap()
        .parse()
        .expect("Invalid control port number");

    let debug_mode = matches.get_flag("debug");

    info!("=== Starting Router {} ===", sysname);
    info!("Interfaces: {:?}", interfaces);
    info!("Listen port: {}", port);
    info!("Control port: {}", control_port);
    info!("Debug mode: {}", debug_mode);

    // Create the network and the routing protocol
    let network = Network::new();
    let protocol = SimpleRoutingProtocol::new(
        sysname,
        interfaces.into_iter().collect::<HashSet<_>>(),
        port,
        network,
        debug_mode,
    ).await?;

    // Create control server and set protocol
    let control_server = Arc::new(ControlServer::new(control_port));
    control_server.set_protocol(protocol).await;

    info!("=== Control Server Commands ===");
    info!("Connect with: telnet 127.0.0.1 {}", control_port);
    info!("Available commands:");
    info!("  {{\"command\": \"status\"}}");
    info!("  {{\"command\": \"neighbors\"}}");
    info!("  {{\"command\": \"neighbors_of\", \"args\": \"ROUTER_ID\"}}");
    info!("  {{\"command\": \"routing_table\"}}");
    info!("  {{\"command\": \"start\"}}");
    info!("  {{\"command\": \"stop\"}}");
    info!("  {{\"command\": \"help\"}}");
    info!("===============================");

    // Start control server task
    let control_server_clone = control_server.clone();
    let control_task = tokio::spawn(async move {
        if let Err(e) = control_server_clone.start().await {
            error!("Control server error: {}", e);
        }
    });

    {
        let protocol_guard = control_server.protocol.lock().await;
        if let Some(protocol_ref) = protocol_guard.as_ref() {
            if let Err(e) = protocol_ref.start_protocol().await {
                error!("Failed to start protocol: {}", e);
                return Err(e);
            }
        }
    }

    info!("Protocol and control server started successfully");

    // Wait for control server
    control_task.await?;

    Ok(())
}