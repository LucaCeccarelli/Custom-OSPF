use rtnetlink::{new_connection, IpVersion};
use std::net::Ipv4Addr;
use anyhow::Result;
use futures::stream::TryStreamExt; 

/// Ajoute une route IPv4 host (/32) vers `dest` via `via`
pub async fn add_ipv4_route(dest: Ipv4Addr, prefix: u8, via: Ipv4Addr) -> Result<()> {
    println!("Ajout route: {}/{} via {}", dest, prefix, via);

    let (connection, handle, _) = new_connection()
        .map_err(|e| anyhow::anyhow!("Erreur connexion netlink: {}", e))?;

    tokio::spawn(connection);

    handle
        .route()
        .add()
        .v4()
        .destination_prefix(dest, prefix)
        .gateway(via)
        .execute()
        .await
        .map_err(|e| anyhow::anyhow!("Erreur ajout route: {}", e))?;

    println!("Route ajoutée avec succès: {}/{} via {}", dest, prefix, via);
    Ok(())
}

/// Supprime une route IPv4 en récupérant d'abord la route existante
pub async fn del_ipv4_route(dest: Ipv4Addr, prefix: u8) -> Result<()> {
    println!("Suppression route: {}/{}", dest, prefix);

    let (connection, handle, _) = new_connection()
        .map_err(|e| anyhow::anyhow!("Erreur connexion netlink: {}", e))?;

    tokio::spawn(connection);

    // Récupérer toutes les routes IPv4
    let mut routes = handle.route().get(IpVersion::V4).execute();

    while let Some(route_msg) = routes.try_next().await
        .map_err(|e| anyhow::anyhow!("Erreur lecture routes: {}", e))? {

        // Vérifier si c'est la route qu'on veut supprimer
        if let Some((dest_addr, prefix_len)) = route_msg.destination_prefix() {
            if dest_addr == std::net::IpAddr::V4(dest) && prefix_len == prefix {
                println!("Route trouvée, suppression...");
                handle.route().del(route_msg).execute().await
                    .map_err(|e| anyhow::anyhow!("Erreur suppression route: {}", e))?;
                println!("Route supprimée: {}/{}", dest, prefix);
                return Ok(());
            }
        }
    }

    println!("Route non trouvée: {}/{} (normal si déjà supprimée)", dest, prefix);
    Ok(())
}
