mod config;
mod interfaces;
mod messages;
mod neighbors;
mod network;
mod routing;
mod netlink;

use std::thread;
use std::time::Duration;

fn main() {
    let cfg = config::load_args();

    println!("Démarrage {} sur interfaces {:?}", cfg.sysname, cfg.interfaces);
    println!("=== DEBUT {} ===", cfg.sysname);

    let ifaces = interfaces::get_interface_ips(&cfg.interfaces);
    let mut neighbor_mgr = neighbors::NeighborManager::new(cfg.sysname.clone());
    let mut network_graph = network::NetworkGraph::new(cfg.sysname.clone());

    let (socket, tx, rx) = messages::start_sockets(&ifaces, cfg.sysname.clone());
    messages::spawn_sender(socket.try_clone().unwrap(), ifaces.clone(), cfg.sysname.clone());
    messages::spawn_receiver(socket, &ifaces, cfg.sysname.clone(), tx.clone());
    loop {
        neighbor_mgr.purge_inactive();
        network_graph.recalculate_routes(&neighbor_mgr, &cfg.sysname);
        thread::sleep(Duration::from_secs(10));
    }
}
