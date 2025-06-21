mod config;
mod interfaces;
mod messages;
mod neighbors;
mod network;
mod routing;
mod netlink;

use std::net::UdpSocket;
use std::thread;
use std::time::Duration;

fn main() {
    let cfg = config::load_args();

    println!("Démarrage {} sur interfaces {:?}", cfg.sysname, cfg.interfaces);
    println!("=== DEBUT {} ===", cfg.sysname);

    // Récupère les IPs des interfaces passées en argument
    let ifaces = interfaces::get_interface_ips(&cfg.interfaces);

    // Crée le gestionnaire de voisins et le graphe réseau
    let mut neighbor_mgr = neighbors::NeighborManager::new(cfg.sysname.clone());
    let mut network_graph = network::NetworkGraph::new(cfg.sysname.clone());

    // Bind socket UDP multicast sur le port 8888
    let socket = UdpSocket::bind("0.0.0.0:8888").expect("Erreur bind socket multicast");

    socket
        .join_multicast_v4(&"224.0.0.1".parse().unwrap(), &"0.0.0.0".parse().unwrap())
        .expect("Erreur join multicast");

    // Lance le sender dans un thread (en supposant spawn_sender adaptée)
    messages::spawn_sender(ifaces.clone(), cfg.sysname.clone());

    // Prépare le canal de communication
    let (tx, rx) = std::sync::mpsc::channel();

    // Lance le receiver dans un thread (adapte les arguments selon ta fonction)
    messages::spawn_receiver(
        socket.try_clone().unwrap(),
        &ifaces,
        cfg.sysname.clone(),
        tx.clone(),
    );

    loop {
        neighbor_mgr.purge_inactive();
        network_graph.recalculate_routes(&neighbor_mgr, &cfg.sysname);
        thread::sleep(Duration::from_secs(10));
    }
}
