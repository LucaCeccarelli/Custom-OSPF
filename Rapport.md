# Rapport Fonctionnel

## Vue d'ensemble

Le protocole développé est une implémentation simplifiée d'un protocole de routage à état de liens, inspiré d'OSPF mais avec des mécanismes adaptés aux contraintes spécifiées. Il est écrit en Rust et utilise une architecture modulaire avec un serveur de contrôle intégré.

## 1. Fonctionnalités de Routage Principal

### 1.1 Calcul des meilleurs chemins

**Fonctionnalités présentes :**
- Calcul basé sur la métrique (nombre de sauts + 1)
- Sélection automatique du meilleur chemin basé sur la métrique la plus faible
- Support des routes directes (métrique 0) et des routes apprises (métrique incrémentée)

**Code pertinent :**
```rust
// Dans route_manager.rs
let route = RouteEntry {
    destination,
    next_hop: actual_next_hop,
    interface: interface_name,
    metric: route_info.metric + 1, // Incrément métrique
    source: RouteSource::Protocol,
};
```

**Limitations identifiées :**
- **Pas de prise en compte des capacités nominales** (débits maximaux)
- Algorithme simplifié basé uniquement sur le nombre de sauts

### 1.2 Mise à jour dynamique des chemins

**Fonctionnalités présentes :**
- Détection automatique des changements de topologie via les messages Hello
- Recalcul automatique lors de la réception de mises à jour de routage
- Mécanisme de failover pour les routes de même métrique

**Code pertinent :**
```rust
// Dans route_manager.rs
if route.metric < existing_route.metric ||
    (route.metric == existing_route.metric && route.next_hop != existing_route.next_hop)
```

- Nettoyage automatique des routes obsolètes via des timeouts configurables
- Suppression des routes lorsqu'un voisin devient inaccessible

### 1.3 Activation/Désactivation à la demande

**Fonctionnalités présentes :**
- Commandes de contrôle via serveur TCP intégré
- API JSON pour démarrer/arrêter le protocole
- Gestion propre des tâches asynchrones

**Code pertinent :**
```rust
// Dans control_server.rs
{"command": "start"}  // Démarre le protocole
{"command": "stop"}   // Arrête le protocole
{"command": "status"} // Vérifie l'état
```

### 1.4 Spécification des interfaces

**Fonctionnalités présentes :**
- Configuration via ligne de commande avec `--interfaces`
- Support de multiples interfaces simultanément
- Découverte automatique des propriétés réseau (IP, masque, broadcast)

**Code pertinent :**
```bash
custom-ospf run --interfaces eth0 --interfaces br0 --sysname R1
```

### 1.5 Modification de la table de routage IPv4

**Fonctionnalités présentes :**
- Intégration directe avec la table de routage système via `net-route`
- Ajout/suppression automatique des routes
- Gestion des erreurs et mécanismes de retry

**Code pertinent :**
```rust
// Dans routing_table.rs
let net_route = net_route::Route::new(
    IpAddr::V4(destination_network),
    prefix_len
).with_gateway(IpAddr::V4(route.next_hop));

match self.route_handle.add(&net_route).await
```

### 1.6 Mémorisation des voisins

**Fonctionnalités présentes :**
- Base de données complète des voisins avec IP, nom système, interfaces
- Suivi temporel (`last_seen`) et état de vie (`is_alive`)
- Persistance en mémoire avec structures thread-safe

**Code pertinent :**
```rust
// Dans types.rs
pub struct NeighborInfo {
    pub router_info: RouterInfo,
    pub last_seen: Instant,
    pub socket_addr: SocketAddr,
    pub is_alive: bool,
}
```

### 1.7 Affichage des voisins à la demande

**Fonctionnalités présentes :**
- Commande pour lister tous les voisins
- Commande pour lister les voisins d'un routeur spécifique
- Mécanisme de requête inter-routeurs avec timeout

**Code pertinent :**
```rust
// Commandes disponibles
{"command": "neighbors"}                            // Tous nos voisins
{"command": "neighbors_of", "args": "ROUTER_ID"}    // Voisins d'un autre routeur
```

**Limitations identifiées :**
La commande permet uniquement d'afficher les voisins d'un routeur voisin à celui utilisé pour exécuter la commande.

### 1.8 Tolérance aux pannes

**Fonctionnalités présentes :**
- Détection automatique des voisins morts via timeout (12 secondes)
- Nettoyage automatique des routes obsolètes (16 secondes)
- Mécanisme de recovery automatique quand les voisins reviennent

**Code pertinent :**
```rust
// Dans protocol/mod.rs - Constantes de timeout
pub const NEIGHBOR_TIMEOUT: Duration = Duration::from_secs(12);
pub const ROUTE_TIMEOUT: Duration = Duration::from_secs(16);
```

## 2. Optimisations et Performance

### 2.1 Minimisation des échanges

**Optimisations présentes :**
- Messages Hello périodiques (4 secondes) plutôt que flooding constant
- Mises à jour de routage périodiques (8 secondes) avec numéro de séquence
- Évitement des boucles par ignorer ses propres messages

**Code pertinent :**
```rust
// Dans protocol/mod.rs - Constantes de pour l'envoie de messages
pub const HELLO_INTERVAL: Duration = Duration::from_secs(4);
pub const UPDATE_INTERVAL: Duration = Duration::from_secs(8);
```

### 2.2 Minimisation mémoire

**Optimisations présentes :**
- Structures de données compactes avec Rust (pas de garbage collector)
- Nettoyage automatique des données obsolètes
- Utilisation d'Arc<Mutex<>> pour partage efficace entre tâches

**Code pertinent :**
```rust
// Nettoyage automatique des requêtes expirées
async fn cleanup_expired_requests(pending_requests: &Arc<Mutex<HashMap<String, PendingNeighborRequest>>>)
```

### 2.3 Minimisation du temps de convergence

**Mécanismes présents :**
- Intervalles courts : Hello (4s), Update (8s), Cleanup (5s)
- Traitement immédiat des messages reçus
- Mise à jour directe de la table de routage système

**Limitations :**
- Pas d'algorithme de convergence rapide (pas de LSA flooding immédiat)
- Pas d'optimisation SPF (Shortest Path First)

## Conclusion

Le protocole implémente avec succès la majorité des fonctionnalités demandées (8/8 pour les fonctionnalités principales, 2/3 partiellement pour les optimisations). Il constitue une base solide et fonctionnelle pour un protocole de routage à état de liens, avec une architecture robuste et extensible. Les principales lacunes concernent l'optimisation des performances et l'intégration de métriques avancées pour le calcul des chemins.