use rtnetlink::new_connection;
use tokio::runtime::Handle;
use std::net::Ipv4Addr;

/// Ajoute une route IPv4 host (/32) vers `dest` via `via`
pub fn add_ipv4_route(dest: Ipv4Addr, prefix: u8, via: Ipv4Addr) {
    // On récupère le Handle courant de Tokio
    let handle = Handle::current();
    handle.spawn(async move {
        // On ouvre une connexion netlink
        let (conn, handle, _) = new_connection().unwrap();
        // On lance le socket netlink en tâche de fond
        tokio::spawn(conn);
        // On ajoute la route
        handle
            .route()
            .add()
            .v4()
            .destination_prefix(dest, prefix)
            .gateway(via)
            .execute()
            .await
            .unwrap();
    });
}
