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

    let rt = Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async {
        let discovery = start_discovery(
            cli.sysname.clone(),
            cli.interfaces.clone(),
            cli.hello_port,
        ).await?;

        // Table des routes installées pour éviter les doublons
        let mut installed_routes: HashSet<(Ipv4Addr, Ipv4Addr)> = HashSet::new();

        loop {
            println!("\n=== Cycle de calcul de routes ===");

            // Affiche l'état actuel des voisins
            {
                let direct_map = discovery.direct.read().await;
                println!("Voisins directs: {:?}", *direct_map);
            }
            {
                let lsa_map = discovery.lsa.read().await;
                println!("LSAs reçus: {:?}", *lsa_map);
            }

            let graph = build_graph(&discovery.lsa).await;
            println!("Graphe construit avec {} nœuds", graph.node_count());

            let routes = compute_best_paths(&graph);
            println!("Routes calculées: {}", routes.len());

            let direct_map = {
                let dm = discovery.direct.read().await;
                dm.clone()
            };

            let mut new_routes: HashSet<(Ipv4Addr, Ipv4Addr)> = HashSet::new();

            for (src_sys, dst_sys, path) in routes {
                println!("Route: {} -> {} via {:?}", src_sys, dst_sys, path);

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
                                println!("Installation route: {} via {}", dst_ip, gw_ip);
                                match netlink::add_ipv4_route(dst_ip, 32, gw_ip).await {
                                    Ok(()) => {
                                        installed_routes.insert((dst_ip, gw_ip));
                                        println!("Route installée avec succès");
                                    }
                                    Err(e) => {
                                        println!("Erreur installation route: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Supprimer les routes qui ne sont plus nécessaires
            let to_remove: Vec<_> = installed_routes.difference(&new_routes).cloned().collect();
            for (dst_ip, _gw_ip) in to_remove {
                println!("Suppression route obsolète: {}", dst_ip);
                match netlink::del_ipv4_route(dst_ip, 32).await {
                    Ok(()) => {
                        installed_routes.remove(&(dst_ip, _gw_ip));
                    }
                    Err(e) => {
                        println!("Erreur suppression route: {}", e);
                    }
                }
            }

            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    })
}
