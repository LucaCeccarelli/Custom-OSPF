mod network;
mod routing;
mod neighbors;
mod netlink;

use clap::Parser;
use tokio::runtime::Builder;
use anyhow::Result;
use std::time::Duration;
use std::net::Ipv4Addr;
use std::collections::HashSet;
use tokio::sync::watch;

use neighbors::start_discovery;
use network::build_graph;
use routing::compute_best_paths;

#[derive(Parser)]
#[command(name = "custom_ospf")]
struct Cli {
    #[arg(long, required = true, num_args = 1..)]
    interfaces: Vec<String>,

    #[arg(long, default_value_t = 5000)]
    hello_port: u16,

    #[arg(long)]
    sysname: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    println!("Démarrage {} sur interfaces {:?}", cli.sysname, cli.interfaces);

    let rt = Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async {
        // Canal watch pour notifier changements topo
        let (tx, mut rx) = watch::channel(());

        let discovery = start_discovery(
            cli.sysname.clone(),
            cli.interfaces.clone(),
            cli.hello_port,
            tx,
        ).await?;

        let mut installed_routes: HashSet<(Ipv4Addr, Ipv4Addr)> = HashSet::new();

        loop {
            // Attendre soit notification de changement soit timeout périodique
            tokio::select! {
                _ = rx.changed() => {
                    println!("\n=== Changement de topologie détecté, nouveau calcul de routes ===");
                }
                _ = tokio::time::sleep(Duration::from_secs(30)) => {
                    println!("\n=== Cycle périodique de recalcul (30s) ===");
                }
            }

            // Verrouiller et cloner pour accès hors lock
            let direct_map = {
                let guard = discovery.direct.read().await;
                guard.clone()
            };

            let lsa_map = {
                let guard = discovery.lsa.read().await;
                guard.clone()
            };

            println!("Voisins directs: {:?}", direct_map);
            println!("LSAs reçus: {:?}", lsa_map);

            let graph = build_graph(&discovery.lsa).await;
            println!("Graphe construit : {} nœuds, {} arêtes", graph.node_count(), graph.edge_count());

            let routes = compute_best_paths(&graph);
            println!("Routes calculées : {}", routes.len());

            let mut new_routes: HashSet<(Ipv4Addr, Ipv4Addr)> = HashSet::new();

            for (src_sys, dst_sys, path) in routes {
                if path.len() >= 2 && src_sys == cli.sysname {
                    let next_hop_sys = &path[1];

                    if let (Some(dst_ip_str), Some(gw_str)) = (
                        direct_map.get(&dst_sys),
                        direct_map.get(next_hop_sys),
                    ) {
                        if let (Ok(dst_ip), Ok(gw_ip)) = (
                            dst_ip_str.parse::<Ipv4Addr>(),
                            gw_str.parse::<Ipv4Addr>(),
                        ) {
                            new_routes.insert((dst_ip, gw_ip));

                            if !installed_routes.contains(&(dst_ip, gw_ip)) {
                                println!("INSTALLATION route: dest {} via {}", dst_ip, gw_ip);
                                match netlink::add_ipv4_route(dst_ip, 32, gw_ip).await {
                                    Ok(()) => {
                                        installed_routes.insert((dst_ip, gw_ip));
                                    }
                                    Err(e) => println!("ERREUR installation route: {}", e),
                                }
                            }
                        }
                    }
                }
            }

            // Supprimer routes obsolètes
            let to_remove: Vec<_> = installed_routes.difference(&new_routes).cloned().collect();
            for (dst_ip, gw_ip) in to_remove {
                println!("SUPPRESSION route obsolète: dest {}", dst_ip);
                match netlink::del_ipv4_route(dst_ip, 32).await {
                    Ok(()) => {
                        installed_routes.remove(&(dst_ip, gw_ip));
                    }
                    Err(e) => println!("ERREUR suppression route: {}", e),
                }
            }
        }
    })
}
