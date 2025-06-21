use clap::{Arg, Command};
use log::info; // Enlever debug si pas utilisé
use std::collections::HashSet;

mod network;
mod protocol;
mod router;
mod routing_table;

use network::Network;
use protocol::SimpleRoutingProtocol;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configuration des logs plus détaillée
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

    let debug_mode = matches.get_flag("debug");

    info!("=== Starting Router {} ===", sysname);
    info!("Interfaces: {:?}", interfaces);
    info!("Listen port: {}", port);
    info!("Debug mode: {}", debug_mode);

    // Créer le réseau et le protocole de routage
    let network = Network::new();
    let mut protocol = SimpleRoutingProtocol::new(
        sysname,
        interfaces.into_iter().collect::<HashSet<_>>(),
        port,
        network,
        debug_mode,
    ).await?;

    // Démarrer le protocole
    protocol.start().await?;

    Ok(())
}
