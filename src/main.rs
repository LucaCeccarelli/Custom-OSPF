use clap::{Args, Parser, Subcommand};
use std::net::IpAddr;
use tracing::{info, error};

mod router;
mod protocol;
mod network;
mod routing_table;
mod neighbor;
mod message;
mod config;

use router::Router;
use config::RouterConfig;

#[derive(Parser)]
#[command(name = "router")]
#[command(about = "Simple Dynamic Routing Protocol")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the routing daemon
    Start(StartArgs),
    /// Control running router
    Control(ControlArgs),
}

#[derive(Args)]
struct StartArgs {
    /// Router ID
    #[arg(short, long)]
    id: String,

    /// Router name
    #[arg(short, long)]
    name: String,

    /// Configuration file path
    #[arg(short, long, default_value = "router.json")]
    config: String,

    /// Control port
    #[arg(long, default_value = "8080")]
    control_port: u16,

    /// Protocol port
    #[arg(long, default_value = "9090")]
    protocol_port: u16,

    /// Set as default router
    #[arg(long)]
    default_router: bool,
}

#[derive(Args)]
struct ControlArgs {
    /// Target router control address
    #[arg(short, long, default_value = "127.0.0.1:8080")]
    target: String,

    #[command(subcommand)]
    action: ControlAction,
}

#[derive(Subcommand)]
enum ControlAction {
    /// Enable the routing protocol
    Enable,
    /// Disable the routing protocol
    Disable,
    /// Show neighbor list
    Neighbors,
    /// Show routing table
    Routes,
    /// Show network topology
    Topology,
    /// Force route recalculation
    Recalculate,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Start(args) => {
            info!("Starting router {} ({})", args.name, args.id);

            // Load configuration
            let config = match RouterConfig::load(&args.config) {
                Ok(config) => {
                    info!("Successfully loaded configuration from {}", args.config);
                    config
                }
                Err(e) => {
                    error!("Failed to load configuration from {}: {}", args.config, e);
                    info!("Using default configuration instead");
                    RouterConfig::default()
                }
            };
            // Create and start router
            let mut router = Router::new(
                args.id,
                args.name,
                config,
                args.control_port,
                args.protocol_port,
                args.default_router,
            ).await?;

            router.start().await?;
        }
        Commands::Control(args) => {
            // Send control commands to running router
            send_control_command(&args.target, args.action).await?;
        }
    }

    Ok(())
}

async fn send_control_command(target: &str, action: ControlAction) -> anyhow::Result<()> {
    use tokio::net::TcpStream;
    use tokio::io::{AsyncWriteExt, AsyncReadExt};

    let mut stream = TcpStream::connect(target).await?;

    let command = match action {
        ControlAction::Enable => "ENABLE",
        ControlAction::Disable => "DISABLE",
        ControlAction::Neighbors => "NEIGHBORS",
        ControlAction::Routes => "ROUTES",
        ControlAction::Topology => "TOPOLOGY",
        ControlAction::Recalculate => "RECALCULATE",
    };

    stream.write_all(command.as_bytes()).await?;
    stream.write_all(b"\n").await?;

    let mut buffer = vec![0; 4096];
    let n = stream.read(&mut buffer).await?;
    let response = String::from_utf8_lossy(&buffer[..n]);

    println!("{}", response);

    Ok(())
}