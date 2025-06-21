use clap::{Arg, Command};
use log::info;
use std::collections::HashSet;
use std::net::Ipv4Addr;

mod network;
mod protocol;
mod router;
mod routing_table;

use network::Network;
use protocol::SimpleRoutingProtocol;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

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

    info!("Starting router {} with interfaces: {:?}", sysname, interfaces);

    // Créer le réseau et le protocole de routage
    let network = Network::new();
    let mut protocol = SimpleRoutingProtocol::new(
        sysname,
        interfaces.into_iter().collect::<HashSet<_>>(),
        port,
        network,
    ).await?;

    // Démarrer le protocole
    protocol.start().await?;

    Ok(())
}
