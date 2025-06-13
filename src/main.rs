use clap::{Parser, Subcommand};
use custom_ospf::*;
use custom_ospf::config::*;
use custom_ospf::protocol::*;
use std::sync::Arc;
use tokio::sync::RwLock;
use log::{info, error};

#[derive(Parser)]
#[command(name = "simple-routing-protocol")]
#[command(about = "A simple routing protocol implementation")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the routing protocol daemon
    Start {
        #[arg(short, long, default_value = "router.json")]
        config: String,
    },
    /// Configure router settings
    Config {
        #[command(subcommand)]
        config_cmd: ConfigCommands,
    },
    /// Show routing information
    Show {
        #[command(subcommand)]
        show_cmd: ShowCommands,
    },
    /// Control protocol state
    Control {
        #[command(subcommand)]
        control_cmd: ControlCommands,
    },
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Create new router configuration
    Init {
        #[arg(short, long)]
        router_id: String,
        #[arg(short, long)]
        name: String,
        #[arg(short, long, default_value = "router.json")]
        output: String,
    },
    /// Add network interface
    AddInterface {
        #[arg(short, long)]
        config: String,
        #[arg(short, long)]
        name: String,
        #[arg(short, long)]
        ip: String,
        #[arg(short, long)]
        network: String,
        #[arg(short, long, default_value = "1000000000")]
        bandwidth: u64,
    },
    /// Set as default router
    SetDefault {
        #[arg(short, long)]
        config: String,
        #[arg(short, long)]
        enabled: bool,
    },
}

#[derive(Subcommand)]
enum ShowCommands {
    /// Show neighbors
    Neighbors {
        #[arg(short, long, default_value = "router.json")]
        config: String,
    },
    /// Show routing table
    Routes {
        #[arg(short, long, default_value = "router.json")]
        config: String,
    },
    /// Show network topology
    Topology {
        #[arg(short, long, default_value = "router.json")]
        config: String,
    },
    /// Show interface status
    Interfaces {
        #[arg(short, long, default_value = "router.json")]
        config: String,
    },
}

#[derive(Subcommand)]
enum ControlCommands {
    /// Enable the routing protocol
    Enable {
        #[arg(short, long, default_value = "router.json")]
        config: String,
    },
    /// Disable the routing protocol
    Disable {
        #[arg(short, long, default_value = "router.json")]
        config: String,
    },
    /// Enable/disable specific interface
    Interface {
        #[arg(short, long, default_value = "router.json")]
        config: String,
        #[arg(short, long)]
        name: String,
        #[arg(short, long)]
        enabled: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Start { config } => {
            start_daemon(config).await?;
        }
        Commands::Config { config_cmd } => {
            handle_config_command(config_cmd).await?;
        }
        Commands::Show { show_cmd } => {
            handle_show_command(show_cmd).await?;
        }
        Commands::Control { control_cmd } => {
            handle_control_command(control_cmd).await?;
        }
    }

    Ok(())
}

async fn start_daemon(config_path: String) -> anyhow::Result<()> {
    info!("Starting routing protocol daemon with config: {}", config_path);

    // Load configuration
    let config = RouterConfig::load_from_file(&config_path)?;

    // Create router state
    let mut router_state = RouterState::new(config.router_id.clone());
    router_state.enabled = config.enabled;
    router_state.is_default_router = config.is_default_router;
    router_state.interfaces = config.to_network_interfaces();

    let shared_state = Arc::new(RwLock::new(router_state));

    // Start protocol engine
    let protocol_engine = ProtocolEngine::new(shared_state.clone()).await?;

    info!("Router {} started", config.router_id);

    // Start the protocol engine (this will run indefinitely)
    protocol_engine.start().await?;

    Ok(())
}

async fn handle_config_command(cmd: ConfigCommands) -> anyhow::Result<()> {
    match cmd {
        ConfigCommands::Init { router_id, name, output } => {
            let config = RouterConfig::new(router_id.clone(), name);
            config.save_to_file(&output)?;
            println!("Router configuration created: {}", output);
            println!("Router ID: {}", router_id);
        }

        ConfigCommands::AddInterface { config, name, ip, network, bandwidth } => {
            let mut router_config = RouterConfig::load_from_file(&config)?;

            let ip_addr: std::net::IpAddr = ip.parse()?;
            let network: ipnetwork::IpNetwork = network.parse()?;

            let interface_config = InterfaceConfig {
                name: name.clone(),
                ip_address: ip_addr,
                network,
                bandwidth,
                enabled: true,
                include_in_routing: true,
            };

            router_config.add_interface(interface_config);
            router_config.save_to_file(&config)?;

            println!("Interface {} added to router configuration", name);
        }

        ConfigCommands::SetDefault { config, enabled } => {
            let mut router_config = RouterConfig::load_from_file(&config)?;
            router_config.is_default_router = enabled;
            router_config.save_to_file(&config)?;

            println!("Default router setting: {}", enabled);
        }
    }

    Ok(())
}

async fn handle_show_command(cmd: ShowCommands) -> anyhow::Result<()> {
    match cmd {
        ShowCommands::Neighbors { config } => {
            show_neighbors(&config).await?;
        }
        ShowCommands::Routes { config } => {
            show_routes(&config).await?;
        }
        ShowCommands::Topology { config } => {
            show_topology(&config).await?;
        }
        ShowCommands::Interfaces { config } => {
            show_interfaces(&config).await?;
        }
    }

    Ok(())
}

async fn handle_control_command(cmd: ControlCommands) -> anyhow::Result<()> {
    match cmd {
        ControlCommands::Enable { config } => {
            let mut router_config = RouterConfig::load_from_file(&config)?;
            router_config.enabled = true;
            router_config.save_to_file(&config)?;
            println!("Routing protocol enabled");
        }

        ControlCommands::Disable { config } => {
            let mut router_config = RouterConfig::load_from_file(&config)?;
            router_config.enabled = false;
            router_config.save_to_file(&config)?;
            println!("Routing protocol disabled");
        }

        ControlCommands::Interface { config, name, enabled } => {
            let mut router_config = RouterConfig::load_from_file(&config)?;
            if let Some(interface) = router_config.interfaces.get_mut(&name) {
                interface.enabled = enabled;
                router_config.save_to_file(&config)?;
                println!("Interface {} {}", name, if enabled { "enabled" } else { "disabled" });
            } else {
                eprintln!("Interface {} not found", name);
            }
        }
    }

    Ok(())
}

async fn show_neighbors(config_path: &str) -> anyhow::Result<()> {
    // In a real implementation, this would connect to the running daemon
    // For now, we'll show what would be displayed
    println!("Neighbors for router:");
    println!("{:<20} {:<15} {:<15} {:<10} {:<15}",
             "Router ID", "IP Address", "Network", "Bandwidth", "Last Seen");
    println!("{}", "-".repeat(80));

    // This is a placeholder - in reality, you'd query the running daemon
    println!("(Connect to running daemon to show live neighbor information)");

    Ok(())
}

async fn show_routes(config_path: &str) -> anyhow::Result<()> {
    println!("Routing Table:");
    println!("{:<18} {:<15} {:<12} {:<8} {:<10}",
             "Destination", "Next Hop", "Interface", "Metric", "Type");
    println!("{}", "-".repeat(70));

    // Placeholder for actual routing table display
    println!("(Connect to running daemon to show live routing table)");

    Ok(())
}

async fn show_topology(config_path: &str) -> anyhow::Result<()> {
    println!("Network Topology:");
    println!("Routers:");
    println!("{:<15} {:<20} {:<30}", "Router ID", "Name", "Networks");
    println!("{}", "-".repeat(70));

    println!("\nLinks:");
    println!("{:<15} {:<15} {:<18} {:<12} {:<8}",
             "From", "To", "Network", "Bandwidth", "Status");
    println!("{}", "-".repeat(75));

    println!("(Connect to running daemon to show live topology)");

    Ok(())
}

async fn show_interfaces(config_path: &str) -> anyhow::Result<()> {
    let config = RouterConfig::load_from_file(config_path)?;

    println!("Network Interfaces for router {}:", config.router_id);
    println!("{:<15} {:<15} {:<18} {:<12} {:<8} {:<10}",
             "Interface", "IP Address", "Network", "Bandwidth", "Enabled", "Routing");
    println!("{}", "-".repeat(85));

    for (_, interface) in &config.interfaces {
        println!("{:<15} {:<15} {:<18} {:<12} {:<8} {:<10}",
                 interface.name,
                 interface.ip_address,
                 interface.network,
                 format_bandwidth(interface.bandwidth),
                 if interface.enabled { "Yes" } else { "No" },
                 if interface.include_in_routing { "Yes" } else { "No" }
        );
    }

    Ok(())
}

fn format_bandwidth(bandwidth: u64) -> String {
    if bandwidth >= 1_000_000_000 {
        format!("{:.1}Gbps", bandwidth as f64 / 1_000_000_000.0)
    } else if bandwidth >= 1_000_000 {
        format!("{:.1}Mbps", bandwidth as f64 / 1_000_000.0)
    } else if bandwidth >= 1_000 {
        format!("{:.1}Kbps", bandwidth as f64 / 1_000.0)
    } else {
        format!("{}bps", bandwidth)
    }
}
