mod network;
mod routing;
mod neighbors;
mod netlink;

use clap::Parser;
use tokio::runtime::Builder;
use anyhow::Result;
use std::time::Duration;
use std::net::Ipv4Addr;

use neighbors::start_discovery;
use network::build_graph;
use routing::compute_best_paths;

#[derive(Parser)]
#[command(name = "custom_ospf",
    about = "Démon OSPF‐like : découverte de voisins, calcul de routes IPv4 et injection via Netlink")]
struct Cli {
    /// IPs des interfaces à utiliser pour HELLO/LSA (au moins une)
    #[arg(long, required = true, num_args = 1..)]
    interfaces: Vec<String>,

    /// Port UDP pour HELLO/LSA
    #[arg(long, default_value_t = 5000)]
    hello_port: u16,

    /// Identifiant unique de ce routeur (sysname)
    #[arg(long)]
    sysname: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Runtime Tokio (current_thread suffit ici)
    let rt = Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async {
        // 1) Démarrage du mécanisme de découverte (HELLO + LSAs)
        let discovery = start_discovery(
            cli.sysname.clone(),
            cli.interfaces.clone(),
            cli.hello_port,
        ).await?;

        loop {
            // 2) Reconstruit le graphe global à partir des LSAs reçus
            let graph = build_graph(&discovery.lsa).await;

            // 3) Calcul des meilleurs chemins (par nombre de sauts / tie‐break capacité)
            let routes = compute_best_paths(&graph);

            // 4) Récupère en une fois la table sysname → IP
            let direct_map = {
                let dm = discovery.direct.read().await;
                dm.clone()  // HashMap<String,String>
            };

            // 5) Pour chaque (src, dst, path), on installe
            //    une route host (/32) vers dst via le premier saut
            for (_src_sys, dst_sys, path) in routes {
                if path.len() >= 2 {
                    let next_hop_sys = &path[1];
                    // on ne peut installer la route que si
                    // dst_sys et next_hop_sys sont dans direct_map
                    if let (Some(dst_ip_str), Some(gw_str)) = (
                        direct_map.get(&dst_sys),
                        direct_map.get(next_hop_sys),
                    ) {
                        // parse des adresses IPv4
                        if let (Ok(dst_ip), Ok(gw_ip)) = (
                            dst_ip_str.parse::<Ipv4Addr>(),
                            gw_str.parse::<Ipv4Addr>(),
                        ) {
                            // préfixe host
                            let prefix = 32;
                            netlink::add_ipv4_route(dst_ip, prefix, gw_ip);
                        }
                    }
                }
            }

            // 6) Attente avant le prochain recalcul
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    })
}
