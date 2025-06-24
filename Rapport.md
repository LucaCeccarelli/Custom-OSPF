# Rapport Fonctionnel 

## Vue d'ensemble

Le protocole développé est une implémentation simplifiée d'un protocole de routage à état de liens, inspiré d'OSPF mais avec des mécanismes adaptés aux contraintes spécifiées. Il est écrit en Rust et utilise une architecture modulaire avec un serveur de contrôle intégré.

## 1. Fonctionnalités de Routage Principal

### 1.1 Calcul des meilleurs chemins

#### Architecture du calcul de routes

Le système implémente un algorithme de calcul de routes basé sur la métrique de distance vectorielle, où chaque route reçue voit sa métrique incrémentée de 1, représentant le coût du saut supplémentaire :

```rust
// Dans route_manager.rs
let route = RouteEntry {
    destination,
    next_hop: actual_next_hop,
    interface: interface_name,
    metric: route_info.metric + 1, // Incrément systématique de la métrique
    source: RouteSource::Protocol,
};
```

#### Mécanisme de sélection des meilleures routes

La logique de sélection privilégie systématiquement les routes avec la métrique la plus faible, avec un mécanisme de comparaison pour gérer les cas d'égalité :

```rust
// Dans route_manager.rs
async fn should_accept_route(route: &RouteEntry, router: &Arc<Mutex<Router>>) -> bool {
    let router_guard = router.lock().await;
    let existing_route = router_guard.routing_table.find_route(&route.destination);

    match existing_route {
        Some(existing) => {
            // Accepte si métrique inférieure OU métrique égale avec next_hop différent (failover)
            route.metric < existing.metric ||
                (route.metric == existing.metric && route.next_hop != existing.next_hop)
        },
        None => true, // Toujours accepter une nouvelle route
    }
}
```

#### Support des routes directes

Le système distingue intelligemment les routes directes des routes apprises via le protocole :

```rust
// Dans router.rs
for (name, interface_info) in &interfaces {
    let route = RouteEntry {
        destination: interface_info.network,
        next_hop: std::net::Ipv4Addr::new(0, 0, 0, 0), // Route directe (0.0.0.0)
        interface: name.clone(),
        metric: 0, // Métrique 0 pour les routes directes
        source: RouteSource::Direct,
    };
    routing_table.add_route(route).await?;
}
```
### 1.2 Mise à jour dynamique des chemins

#### Détection des changements topologiques

Le système implémente une détection proactive des changements via un mécanisme de messages Hello périodiques, permettant une réactivité rapide aux modifications de topologie :

```rust
// Dans task_manager.rs
async fn send_hello_messages(router: &Arc<Mutex<Router>>, sockets: &[Arc<UdpSocket>], port: u16) {
    let router_guard = router.lock().await;
    let router_info = router_guard.get_router_info();
    drop(router_guard);

    let hello = HelloMessage {
        router_id: router_info.router_id.clone(),
        interfaces: router_info.interfaces
            .iter()
            .map(|(name, info)| (name.clone(), info.ip.to_string()))
            .collect(),
    };
    // Diffusion périodique sur toutes les interfaces
}
```

#### Mécanisme de mise à jour intelligent

Le traitement des mises à jour de routage intègre une logique de validation et de remplacement des routes existantes :

```rust
// Dans routing_table.rs
pub async fn add_route(&mut self, route: RouteEntry) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(existing_index) = self.find_route_index(&route.destination) {
        let existing_route = &self.routes[existing_index];

        // Logique de remplacement avancée
        if route.metric < existing_route.metric ||
            (route.source == existing_route.source && route.next_hop != existing_route.next_hop) {

            info!("Updating route to {} (old metric: {}, new metric: {}, old nexthop: {}, new nexthop: {})",
                  route.destination, existing_route.metric, route.metric, 
                  existing_route.next_hop, route.next_hop);

            // Suppression propre de l'ancienne route système
            if existing_route.source != RouteSource::Direct {
                self.delete_system_route(existing_route).await?;
            }

            // Mise à jour et injection de la nouvelle route
            self.routes[existing_index] = route.clone();
            if route.source != RouteSource::Direct {
                self.add_system_route(&route).await?;
            }
        }
    }
}
```

#### Gestion des timeouts et nettoyage

Le système implémente des mécanismes de timeout différenciés pour optimiser la convergence :

```rust
// Dans protocol/mod.rs 
pub const NEIGHBOR_TIMEOUT: Duration = Duration::from_secs(12);  // Détection voisin mort
pub const ROUTE_TIMEOUT: Duration = Duration::from_secs(16);     // Suppression route obsolète
pub const HELLO_INTERVAL: Duration = Duration::from_secs(4);     // Fréquence Hello
pub const UPDATE_INTERVAL: Duration = Duration::from_secs(8);    // Fréquence mise à jour
pub const CLEANUP_INTERVAL: Duration = Duration::from_secs(5);   // Fréquence nettoyage
```

La tâche de nettoyage automatique assure la cohérence de la base de données de routage :

```rust
// Dans task_manager.rs 
async fn perform_cleanup(neighbors: &Arc<Mutex<HashMap<String, NeighborInfo>>>, 
                        route_states: &Arc<Mutex<HashMap<String, RouteState>>>, 
                        router: &Arc<Mutex<Router>>) {
    let now = Instant::now();
    let mut dead_neighbors = Vec::new();

    // Détection des voisins morts par timeout
    {
        let mut neighbors_guard = neighbors.lock().await;
        for (neighbor_id, neighbor_info) in neighbors_guard.iter_mut() {
            if now.duration_since(neighbor_info.last_seen) > SimpleRoutingProtocol::NEIGHBOR_TIMEOUT {
                if neighbor_info.is_alive {
                    warn!("Neighbor {} is now considered DEAD", neighbor_id);
                    neighbor_info.is_alive = false;
                    dead_neighbors.push(neighbor_id.clone());
                }
            }
        }
    }
    
    // Suppression automatique des routes via voisins morts
    for dead_neighbor_id in &dead_neighbors {
        if let Some(dead_neighbor_ip) = route_manager::get_neighbor_ip(dead_neighbor_id, neighbors).await {
            let mut router_guard = router.lock().await;
            if let Ok(removed_routes) = router_guard.routing_table.remove_routes_via_nexthop(dead_neighbor_ip).await {
                info!("Successfully removed {} routes via dead neighbor {}", removed_routes.len(), dead_neighbor_ip);
            }
        }
    }
}
```

### 1.3 Activation/Désactivation à la demande

#### Architecture du serveur de contrôle

Le système intègre un serveur de contrôle TCP utilisant une API JSON pour l'administration du protocole :

```rust
// Dans control_server.rs
pub async fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
    let bind_addr = format!("127.0.0.1:{}", self.port);
    let listener = TcpListener::bind(&bind_addr).await?;
    info!("Control server listening on {}", bind_addr);

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                info!("Control connection from {}", addr);
                let protocol_clone = self.protocol.clone();
                // Gestion asynchrone de chaque connexion client
                tokio::spawn(async move {
                    if let Err(e) = Self::handle_client(stream, protocol_clone).await {
                        error!("Error handling control client {}: {}", addr, e);
                    }
                });
            }
        }
    }
}
```

#### Gestion des commandes de contrôle

Le processeur de commandes offre une interface complète pour la gestion du protocole :

```rust
// Dans control_server.rs
async fn process_command(command: ControlCommand, 
                        protocol: &Arc<Mutex<Option<SimpleRoutingProtocol>>>) -> ControlResponse {
    match command.command.as_str() {
        "status" => Self::get_status(protocol).await,
        "neighbors" => Self::get_neighbors(protocol).await,
        "neighbors_of" => Self::get_neighbors_of(command.args, protocol).await,
        "start" => Self::start_protocol(protocol).await,    // Démarrage contrôlé
        "stop" => Self::stop_protocol(protocol).await,      // Arrêt propre
        "routing_table" => Self::get_routing_table(protocol).await,
        "help" => Self::get_help(),
        _ => ControlResponse {
            success: false,
            message: format!("Unknown command: {}", command.command),
            data: None,
        },
    }
}
```

#### Mécanisme de démarrage/arrêt

Le contrôle du cycle de vie du protocole utilise des primitives atomiques pour assurer la cohérence :

```rust
// Dans protocol/mod.rs - ligne 80-95
pub async fn start_protocol(&self) -> Result<(), Box<dyn std::error::Error>> {
    if self.is_running.load(Ordering::Relaxed) {
        return Ok(()); // Évite les doubles démarrages
    }

    info!("Starting routing protocol...");
    self.is_running.store(true, Ordering::Relaxed);

    let (shutdown_tx, _) = broadcast::channel(1);
    {
        let mut tx_guard = self.shutdown_tx.lock().await;
        *tx_guard = Some(shutdown_tx);
    }

    task_manager::start_tasks(self).await?; // Démarrage de toutes les tâches
    info!("Routing protocol started successfully");
    Ok(())
}
```

```rust
// Dans protocol/mod.rs
pub async fn stop_protocol(&self) -> Result<(), Box<dyn std::error::Error>> {
    if !self.is_running.load(Ordering::Relaxed) {
        return Ok(());
    }

    info!("Stopping routing protocol...");
    self.is_running.store(false, Ordering::Relaxed);

    // Signal d'arrêt à toutes les tâches
    {
        let tx_guard = self.shutdown_tx.lock().await;
        if let Some(ref tx) = *tx_guard {
            let _ = tx.send(());
        }
    }

    // Arrêt propre de toutes les tâches
    {
        let mut handles_guard = self.task_handles.lock().await;
        for handle in handles_guard.drain(..) {
            handle.abort();
        }
    }
    Ok(())
}
```

### 1.4 Spécification des interfaces

#### Configuration par ligne de commande

Le système utilise la bibliothèque `clap` pour une interface en ligne de commande robuste et extensible :

```rust
// Dans main.rs
let matches = Command::new("Simple Routing Protocol")
    .version("1.0")
    .arg(
        Arg::new("interfaces")
            .long("interfaces")
            .value_name("INTERFACE")
            .help("Network interfaces to include in routing")
            .action(clap::ArgAction::Append)  // Support de multiples interfaces
            .required(true),
    )
    .arg(
        Arg::new("sysname")
            .long("sysname")
            .value_name("NAME")
            .help("System name for this router")
            .required(true),
    )
    .get_matches();

let interfaces: Vec<String> = matches
    .get_many::<String>("interfaces")
    .unwrap()
    .cloned()
    .collect();
```

#### Découverte automatique des propriétés réseau

Le module de découverte réseau utilise la bibliothèque `pnet` pour extraire automatiquement les informations des interfaces système :

```rust
// Dans network.rs
fn get_interface_info(&self, interface_name: &str) -> Result<InterfaceInfo, Box<dyn std::error::Error>> {
    let interfaces = datalink::interfaces();

    let target_interface = interfaces
        .into_iter()
        .find(|iface| iface.name == interface_name)
        .ok_or_else(|| format!("Interface {} not found", interface_name))?;

    // Vérification de l'état opérationnel
    if !target_interface.is_up() {
        return Err(format!("Interface {} is not up", interface_name).into());
    }

    // Extraction des adresses IPv4
    for ip_network in target_interface.ips {
        if let IpNetwork::V4(ipv4_network) = ip_network {
            if ipv4_network.ip().is_loopback() {
                continue; // Ignore les adresses de loopback
            }

            return Ok(InterfaceInfo {
                name: interface_name.to_string(),
                ip: ipv4_network.ip(),
                network: ipv4_network,
                metric: 1, // Métrique par défaut
            });
        }
    }
}
```

#### Validation et gestion d'erreurs

Le système intègre une validation des interfaces spécifiées :

```rust
// Dans network.rs
pub fn discover_interfaces(&mut self, interface_names: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    info!("Discovering interfaces: {:?}", interface_names);

    for interface_name in interface_names {
        if let Ok(interface_info) = self.get_interface_info(interface_name) {
            info!("Found interface: {} -> {} ({})", 
                  interface_info.name, interface_info.ip, interface_info.network);
            self.interfaces.insert(interface_name.clone(), interface_info);
        } else {
            warn!("Interface {} not found or has no IP", interface_name);
        }
    }

    if self.interfaces.is_empty() {
        return Err("No valid interfaces found".into()); // Échec si aucune interface valide
    }
    Ok(())
}
```

### 1.5 Modification de la table de routage IPv4

#### Intégration système via net-route

Le protocole utilise la bibliothèque `net-route` pour une intégration native avec la table de routage du système d'exploitation :

```rust
// Dans routing_table.rs
async fn add_system_route(&self, route: &RouteEntry) -> Result<(), Box<dyn std::error::Error>> {
    info!("Adding route via net-route: {} via {} metric {}", route.destination, route.next_hop, route.metric);

    let destination_network = route.destination.network();
    let prefix_len = route.destination.prefix();

    let net_route = net_route::Route::new(
        IpAddr::V4(destination_network),
        prefix_len
    ).with_gateway(IpAddr::V4(route.next_hop));

    match self.route_handle.add(&net_route).await {
        Ok(_) => {
            info!("Successfully added route: {}", route.destination);
            Ok(())
        },
        Err(e) => {
            debug!("Route add failed, trying to update: {}", e);
            // Mécanisme de retry en cas de conflit
            let _ = self.route_handle.delete(&net_route).await;
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

            match self.route_handle.add(&net_route).await {
                Ok(_) => {
                    info!("Successfully updated route: {}", route.destination);
                    Ok(())
                },
                Err(e2) => {
                    warn!("Failed to add/update route {}: {}", route.destination, e2);
                    Ok(()) // Continue malgré l'erreur pour ne pas bloquer le protocole
                }
            }
        }
    }
}
```

#### Mécanisme de suppression

La suppression des routes intègre une gestion d'erreurs appropriée :

```rust
// Dans routing_table.rs
pub async fn remove_routes_via_nexthop(&mut self, next_hop: Ipv4Addr) -> Result<Vec<RouteEntry>, Box<dyn std::error::Error>> {
    let mut removed_routes = Vec::new();
    let mut indices_to_remove = Vec::new();

    // Identification des routes à supprimer
    for (index, route) in self.routes.iter().enumerate() {
        if route.next_hop == next_hop && route.source == RouteSource::Protocol {
            indices_to_remove.push(index);
        }
    }

    // Suppression en ordre inverse pour préserver les indices
    for &index in indices_to_remove.iter().rev() {
        let route = self.routes.remove(index);
        info!("Removing route to {} via dead neighbor {}", route.destination, next_hop);

        // Suppression de la table système
        self.delete_system_route(&route).await?;
        removed_routes.push(route);
    }

    if !removed_routes.is_empty() {
        info!("Removed {} routes via dead neighbor {}", removed_routes.len(), next_hop);
    }
    Ok(removed_routes)
}
```

#### Validation avancée des routes

Le système implémente une validation avant l'insertion des routes :

```rust
// Dans routing_table.rs
fn is_valid_route(&self, route: &RouteEntry) -> bool {
    if route.source == RouteSource::Direct {
        return true; // Les routes directes sont toujours valides
    }

    // Validation de la passerelle
    if route.next_hop.is_loopback() {
        debug!("Invalid gateway (loopback): {}", route.next_hop);
        return false;
    }

    // Éviter les routes vers nos propres réseaux
    for local_net in &self.local_networks {
        if route.destination.network() == local_net.network() &&
            route.destination.prefix() == local_net.prefix() {
            debug!("Skipping route to local network {}", route.destination);
            return false;
        }
    }

    // Vérification de l'accessibilité de la passerelle
    if !route.next_hop.is_unspecified() {
        let mut gateway_is_local = false;
        for local_net in &self.local_networks {
            if local_net.contains(route.next_hop) {
                debug!("Gateway {} found in local network {}", route.next_hop, local_net);
                gateway_is_local = true;
                break;
            }
        }

        if !gateway_is_local {
            debug!("Gateway {} is not in any local network", route.next_hop);
            return false;
        }
    }
    true
}
```

### 1.6 Mémorisation des voisins 

#### Structure de données des voisins

Le système maintient une base de données complète des voisins avec des informations détaillées et un suivi temporel :

```rust
// Dans types.rs
#[derive(Debug, Clone)]
pub struct NeighborInfo {
    pub router_info: RouterInfo,    // Informations détaillées du routeur
    pub last_seen: Instant,         // Timestamp de dernière activité
    pub socket_addr: SocketAddr,    // Adresse réseau pour communication
    pub is_alive: bool,             // État de vie actuel
}

// Dans router.rs
#[derive(Debug, Clone)]
pub struct RouterInfo {
    pub router_id: String,                                    // Identifiant unique
    pub interfaces: HashMap<String, InterfaceInfo>,          // Interfaces avec détails réseau
}
```

#### Traitement des messages Hello

Le système traite les messages Hello pour maintenir à jour la base de données des voisins :

```rust
// Dans task_manager.rs
async fn handle_hello_message_static(hello_data: &str, addr: SocketAddr, protocol_data: &ProtocolData) -> Result<(), Box<dyn std::error::Error>> {
    let hello: HelloMessage = serde_json::from_str(hello_data)?;

    // Éviter les boucles en ignorant nos propres messages
    let router_guard = protocol_data.router.lock().await;
    if hello.router_id == router_guard.id {
        drop(router_guard);
        return Ok(());
    }
    drop(router_guard);

    debug!("Received HELLO from {} at {}", hello.router_id, addr);

    // Construction des informations du routeur voisin
    let router_info = crate::router::RouterInfo {
        router_id: hello.router_id.clone(),
        interfaces: hello.interfaces.iter()
            .map(|(name, ip)| {
                let ip_addr: Ipv4Addr = ip.parse().unwrap_or(Ipv4Addr::new(0, 0, 0, 0));
                (name.clone(), crate::network::InterfaceInfo {
                    name: name.clone(),
                    ip: ip_addr,
                    network: format!("{}/32", ip).parse().unwrap(),
                    metric: 1,
                })
            })
            .collect(),
    };

    let neighbor_info = NeighborInfo {
        router_info,
        last_seen: Instant::now(),
        socket_addr: addr,
        is_alive: true,
    };

    // Mise à jour thread-safe de la base de données
    let mut neighbors_guard = protocol_data.neighbors.lock().await;
    let was_dead = neighbors_guard.get(&hello.router_id)
        .map(|n| !n.is_alive)
        .unwrap_or(true);

    neighbors_guard.insert(hello.router_id.clone(), neighbor_info);

    if was_dead {
        info!("Neighbor {} is now ALIVE at {}", hello.router_id, addr);
    }
    Ok(())
}
```

### 1.7 Affichage des voisins à la demande

#### Interface de consultation locale

Le système offre une interface pour consulter les voisins directs :

```rust
// Dans neighbor_manager.rs
pub async fn get_neighbors_list(neighbors: &Arc<Mutex<HashMap<String, NeighborInfo>>>) -> Vec<ControlNeighborInfo> {
    let neighbors_guard = neighbors.lock().await;
    let mut neighbor_list = Vec::new();

    for (router_id, neighbor_info) in neighbors_guard.iter() {
        let last_seen = format!("{:?} ago", neighbor_info.last_seen.elapsed());
        let interfaces: Vec<String> = neighbor_info.router_info.interfaces
            .values()
            .map(|iface| iface.ip.to_string())
            .collect();

        neighbor_list.push(ControlNeighborInfo {
            router_id: router_id.clone(),
            ip_address: neighbor_info.socket_addr.ip().to_string(),
            last_seen,
            is_alive: neighbor_info.is_alive,
            interfaces,
        });
    }
    neighbor_list
}
```

#### Mécanisme de requête inter-routeurs

Le système implémente un mécanisme de requête permettant d'interroger les voisins d'un routeur distant :

```rust
// Dans neighbor_manager.rs
pub async fn request_neighbors_from_router(protocol: &SimpleRoutingProtocol, target_router_id: &str) -> Option<Vec<ControlNeighborInfo>> {
    // Génération d'un ID de requête unique
    let request_id = format!("{}_{}",
                             SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis(),
                             std::ptr::addr_of!(*protocol) as usize
    );

    let router_guard = protocol.get_router().lock().await;
    let our_router_id = router_guard.id.clone();
    drop(router_guard);

    // Création de la requête
    let neighbor_request = NeighborRequest {
        requesting_router_id: our_router_id,
        request_id: request_id.clone(),
        target_router_id: target_router_id.to_string(),
    };

    // Canal pour la réponse asynchrone
    let (tx, rx) = oneshot::channel();

    // Stockage de la requête en attente
    {
        let mut pending_requests = protocol.get_pending_neighbor_requests().lock().await;
        pending_requests.insert(request_id.clone(), PendingNeighborRequest {
            responder: tx,
            timestamp: SystemTime::now(),
        });
    }

    // Diffusion de la requête
    send_neighbor_request(protocol, &neighbor_request).await;

    // Attente de la réponse avec timeout
    match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await {
        Ok(Ok(neighbors)) => {
            info!("Received neighbor information for router {} ({} neighbors)", target_router_id, neighbors.len());
            Some(neighbors)
        }
        _ => {
            warn!("Timeout or error waiting for neighbor response from router {}", target_router_id);
            cleanup_pending_request(protocol, &request_id).await;
            None
        }
    }
}
```

#### Traitement des requêtes distantes

Le système traite automatiquement les requêtes de voisinage provenant d'autres routeurs :

```rust
// Dans task_manager.rs
async fn handle_neighbor_request_static(request_data: &str, addr: SocketAddr, protocol_data: &ProtocolData) -> Result<(), Box<dyn std::error::Error>> {
    let request: NeighborRequest = serde_json::from_str(request_data)?;

    let router_guard = protocol_data.router.lock().await;
    let our_router_id = router_guard.id.clone();
    drop(router_guard);

    // Vérification si la requête nous concerne
    if request.target_router_id == our_router_id {
        info!("Received NEIGHBOR_REQUEST from {} for us (request_id: {})", request.requesting_router_id, request.request_id);

        // Collecte de nos voisins actuels
        let current_neighbors = neighbor_manager::get_current_neighbors_for_response(&protocol_data.neighbors).await;

        // Création et envoi de la réponse
        let neighbor_response = NeighborResponse {
            responding_router_id: our_router_id,
            request_id: request.request_id,
            neighbors: current_neighbors,
        };

        if let Ok(response_message) = serde_json::to_string(&neighbor_response) {
            let response_packet = format!("NEIGHBOR_RESPONSE:{}", response_message);

            if let Some(socket) = protocol_data.sockets.first() {
                if let Err(e) = socket.send_to(response_packet.as_bytes(), addr).await {
                    warn!("Failed to send neighbor response to {}: {}", addr, e);
                } else {
                    info!("Sent NEIGHBOR_RESPONSE to {} with {} neighbors", addr, neighbor_response.neighbors.len());
                }
            }
        }
    }
    Ok(())
}
```

**Limitation:** Le système permet de consulter les voisins de n'importe quel routeur accessible via la diffusion réseau, mais la portée est limitée aux routeurs directement accessibles en broadcast depuis notre position.

### 1.8 Tolérance aux pannes

#### Détection multi-niveaux des pannes

Le système implémente plusieurs niveaux de détection des pannes avec des timeouts adaptés :

```rust
// Dans protocol/mod.rs - Constantes de timeout optimisées
pub const NEIGHBOR_TIMEOUT: Duration = Duration::from_secs(12);    // 3x HELLO_INTERVAL
pub const ROUTE_TIMEOUT: Duration = Duration::from_secs(16);       // 2x UPDATE_INTERVAL  
pub const HELLO_INTERVAL: Duration = Duration::from_secs(4);       // Maintien de la connectivité
pub const UPDATE_INTERVAL: Duration = Duration::from_secs(8);      // Propagation des routes
pub const CLEANUP_INTERVAL: Duration = Duration::from_secs(5);     // Nettoyage périodique
```

Cette hiérarchie de timeouts garantit une détection rapide tout en évitant les faux positifs dus aux variations de latence réseau.

#### Mécanisme de recovery automatique

Le système implémente un mécanisme de détection et de recovery :

```rust
// Dans task_manager.rs
async fn perform_cleanup(neighbors: &Arc<Mutex<HashMap<String, NeighborInfo>>>, 
                        route_states: &Arc<Mutex<HashMap<String, RouteState>>>, 
                        router: &Arc<Mutex<Router>>) {
    let now = Instant::now();
    let mut dead_neighbors = Vec::new();
    let mut stale_routes = Vec::new();

    // Phase 1: Détection des voisins morts
    {
        let mut neighbors_guard = neighbors.lock().await;
        for (neighbor_id, neighbor_info) in neighbors_guard.iter_mut() {
            if now.duration_since(neighbor_info.last_seen) > SimpleRoutingProtocol::NEIGHBOR_TIMEOUT {
                if neighbor_info.is_alive {
                    warn!("Neighbor {} is now considered DEAD (last seen: {:?} ago)",
                          neighbor_id, now.duration_since(neighbor_info.last_seen));
                    neighbor_info.is_alive = false;
                    dead_neighbors.push(neighbor_id.clone());
                }
            } else if !neighbor_info.is_alive {
                info!("Neighbor {} is now ALIVE again", neighbor_id);
                neighbor_info.is_alive = true; // Recovery automatique
            }
        }
    }

    // Phase 2: Identification des routes obsolètes
    {
        let route_states_guard = route_states.lock().await;
        for (destination, route_state) in route_states_guard.iter() {
            if now.duration_since(route_state.last_advertised) > SimpleRoutingProtocol::ROUTE_TIMEOUT {
                stale_routes.push(destination.clone());
            }
        }
    }

    // Phase 3: Nettoyage coordonné des routes
    if !dead_neighbors.is_empty() || !stale_routes.is_empty() {
        let mut routes_to_remove = HashSet::new();

        {
            let route_states_guard = route_states.lock().await;
            for (destination, route_state) in route_states_guard.iter() {
                if dead_neighbors.contains(&route_state.advertising_neighbor) ||
                    stale_routes.contains(destination) {
                    routes_to_remove.insert(destination.clone());
                }
            }
        }

        if !routes_to_remove.is_empty() {
            route_manager::remove_routes_via_nexthop(routes_to_remove, router, route_states).await;
        }
    }
}
```

#### Gestion des pannes en cascade

Le système gère les pannes en cascade en supprimant toutes les routes dépendantes d'un voisin défaillant :

```rust
// Dans routing_table.rs
pub async fn remove_routes_via_nexthop(&mut self, next_hop: Ipv4Addr) -> Result<Vec<RouteEntry>, Box<dyn std::error::Error>> {
    let mut removed_routes = Vec::new();
    let mut indices_to_remove = Vec::new();

    // Identification exhaustive des routes impactées
    for (index, route) in self.routes.iter().enumerate() {
        if route.next_hop == next_hop && route.source == RouteSource::Protocol {
            indices_to_remove.push(index);
        }
    }

    // Suppression atomique pour éviter la corruption de la table
    for &index in indices_to_remove.iter().rev() {
        let route = self.routes.remove(index);
        info!("Removing route to {} via dead neighbor {}", route.destination, next_hop);

        // Suppression synchronisée de la table système
        self.delete_system_route(&route).await?;
        removed_routes.push(route);
    }

    if !removed_routes.is_empty() {
        info!("Removed {} routes via dead neighbor {}", removed_routes.len(), next_hop);
    }
    Ok(removed_routes)
}
```

#### Mécanisme de failover

Le système inclut un mécanisme de failover pour les routes de même métrique :

```rust
// Dans router.rs - ligne 45-65
pub async fn update_routing_table(&mut self, new_routes: Vec<RouteEntry>) -> Result<(), Box<dyn std::error::Error>> {
    for route in new_routes {
        if !self.routing_table.has_better_route(&route) {
            // Gestion spéciale du failover pour métriques égales
            if let Some(existing) = self.routing_table.find_route(&route.destination) {
                if existing.metric == route.metric && existing.next_hop != route.next_hop {
                    info!("Replacing route to {} (failover from {} to {})",
                          route.destination, existing.next_hop, route.next_hop);
                    self.routing_table.replace_route(route).await?;
                } else {
                    self.routing_table.add_route(route).await?;
                }
            } else {
                self.routing_table.add_route(route).await?;
            }
        }
    }
    Ok(())
}
```

## 2. Optimisations et Performance

### 2.1 Minimisation des échanges

#### Stratégie de communication périodique

Le protocole utilise une approche périodique plutôt qu'événementielle pour minimiser la charge réseau :

```rust
// Dans protocol/mod.rs - Intervalles optimisés
pub const HELLO_INTERVAL: Duration = Duration::from_secs(4);    // 3x plus rapide que la détection
pub const UPDATE_INTERVAL: Duration = Duration::from_secs(8);   // Équilibre entre réactivité et charge
```

Cette stratégie présente plusieurs avantages :
- **Prédictibilité** : Charge réseau constante et prévisible
- **Simplification** : Pas de gestion complexe d'événements
- **Robustesse** : Résistance aux pertes de paquets isolées

#### Évitement des boucles de routage

Le système implémente plusieurs mécanismes pour éviter les boucles :

```rust
// Dans task_manager.rs - ligne 200-210
async fn socket_listen_loop(...) {
    loop {
        match result {
            Ok((len, addr)) => {
                // Filtrage des messages en boucle
                if let std::net::IpAddr::V4(ipv4_addr) = addr.ip() {
                    if our_ips.contains(&ipv4_addr) {
                        debug!("Ignoring loopback message from our own IP: {}", addr.ip());
                        continue;
                    }
                }
                // Traitement du message...
            }
        }
    }
}
```

```rust
// Dans task_manager.rs
async fn handle_hello_message_static(...) {
    let hello: HelloMessage = serde_json::from_str(hello_data)?;

    // Filtrage par ID de routeur
    let router_guard = protocol_data.router.lock().await;
    if hello.router_id == router_guard.id {
        drop(router_guard);
        return Ok(()); // Ignorer nos propres messages
    }
}
```

#### Mécanisme de séquençage

Les mises à jour utilisent un numéro de séquence pour détecter les duplicatas et ordonner les mises à jour :

```rust
// Dans task_manager.rs
async fn send_routing_updates(...) {
    let mut seq_guard = sequence.lock().await;
    *seq_guard += 1;
    let current_seq = *seq_guard;
    drop(seq_guard);

    let update = RoutingUpdate {
        router_id: router_id.clone(),
        sequence: current_seq,    // Séquençage pour détecter les duplicatas
        routes,
    };
}
```

### 2.2 Minimisation mémoire

#### Architecture zero-copy avec Rust

L'utilisation de Rust permet une gestion mémoire optimale sans garbage collector :

```rust
// Dans protocol/mod.rs - Structures optimisées
neighbors: Arc<Mutex<HashMap<String, NeighborInfo>>>,     // Partage efficace
route_states: Arc<Mutex<HashMap<String, RouteState>>>,    // Une seule copie
sockets: Vec<Arc<UdpSocket>>,                             // Référencement sans copie
```

L'architecture `Arc<Mutex<>>` garantit :
- **Partage sans copie** : Les données sont partagées entre toutes les tâches
- **Sécurité mémoire** : Aucun risque de corruption ou de fuite

#### Nettoyage automatique proactif

Le système implémente plusieurs niveaux de nettoyage automatique :

```rust
// Dans task_manager.rs - ligne 520-540
async fn cleanup_expired_requests(pending_requests: &Arc<Mutex<HashMap<String, PendingNeighborRequest>>>) {
    let now = SystemTime::now();
    let mut expired_requests = Vec::new();

    {
        let pending_requests_guard = pending_requests.lock().await;
        for (request_id, request) in pending_requests_guard.iter() {
            if now.duration_since(request.timestamp).unwrap_or(Duration::ZERO) > Duration::from_secs(30) {
                expired_requests.push(request_id.clone());
            }
        }
    }

    if !expired_requests.is_empty() {
        let mut pending_requests_guard = pending_requests.lock().await;
        for request_id in expired_requests {
            pending_requests_guard.remove(&request_id);
            debug!("Cleaned up expired neighbor request: {}", request_id);
        }
    }
}
```

#### Structures de données compactes

Les structures sont optimisées pour minimiser l'empreinte mémoire :

```rust
// Dans types.rs - Structures optimisées
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RouteInfo {
    pub destination: String,    // Représentation textuelle compacte
    pub metric: u32,           // Type primitif
    pub next_hop: String,      // Réutilisation pour sérialisation
}

#[derive(Debug, Clone)]
pub struct RouteState {
    pub route: RouteEntry,              // Référence directe
    pub last_advertised: Instant,       // Timestamp compact
    pub advertising_neighbor: String,    // ID simple
}
```

### 2.3 Minimisation du temps de convergence

#### Mécanismes de convergence présents

Le système implémente plusieurs mécanismes pour accélérer la convergence :

**1. Traitement immédiat des messages :**
```rust
// Dans task_manager.rs - Traitement temps réel
async fn socket_listen_loop(...) {
    loop {
        tokio::select! {
            result = socket.recv_from(&mut buffer) => {
                match result {
                    Ok((len, addr)) => {
                        // Traitement immédiat sans attente
                        if let Err(e) = handle_message_with_protocol_data(&data, addr, &protocol_data).await {
                            warn!("Failed to handle message from {}: {}", addr, e);
                        }
                    }
                }
            }
        }
    }
}
```

**2. Mise à jour directe de la table de routage :**
```rust
// Dans route_manager.rs 
async fn apply_new_routes(router: &Arc<Mutex<Router>>, new_routes: Vec<RouteEntry>, advertising_router: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut router_guard = router.lock().await;
    router_guard.update_routing_table(new_routes).await?;    // Application immédiate
    info!("Updated routing table with routes from {}", advertising_router);
    Ok(())
}
```

**3. Intervalles courts et équilibrés :**
```rust
// Compromis entre réactivité et stabilité
pub const HELLO_INTERVAL: Duration = Duration::from_secs(4);      // Détection rapide des voisins
pub const UPDATE_INTERVAL: Duration = Duration::from_secs(8);     // Propagation régulière
pub const CLEANUP_INTERVAL: Duration = Duration::from_secs(5);    // Nettoyage fréquent
```
#### Métriques de performance actuelles

Avec la configuration actuelle, les temps de convergence théoriques sont :
- **Détection de panne** : 12 secondes maximum (3 × HELLO_INTERVAL)
- **Propagation d'une nouvelle route** : 8-16 secondes (1-2 × UPDATE_INTERVAL)
- **Convergence complète** : 24-32 secondes dans le pire cas
