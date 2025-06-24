# Custom OSPF - Protocole de Routage Dynamique

Un protocole de routage à état de liens inspiré d'OSPF, implémenté en Rust pour la découverte automatique de voisins, l'échange de routes et la mise à jour dynamique des tables de routage.

## Installation

```bash
# Compiler
cargo build --release

# Ou installer directement
cargo install --path .
```

## Utilisation

### Démarrage du Protocole

```bash
custom-ospf run \
  --interfaces eth0 \
  --interfaces br0 \
  --sysname R1 \
  --control-port 8080 \
  --debug
```

#### Paramètres obligatoires
- `--interfaces <INTERFACE>` : Interface(s) réseau à inclure dans le protocole (répétable)
- `--sysname <NAME>` : Identifiant unique du routeur

#### Paramètres optionnels
- `--listen-port <PORT>` : Port d'écoute pour les messages du protocole (défaut: 5555)
- `--control-port <PORT>` : Port du serveur de contrôle (défaut: 8080)
- `--debug` : Active l'affichage périodique de l'état (toutes les 30 secondes)

#### Remarque
La commande doit être exécutée avec les privilèges root pour accéder aux interfaces réseau et modifier la table de routage.

## Interface de Contrôle

### Connexion au Serveur de Contrôle

```bash
# Via telnet
telnet 127.0.0.1 8080

# Via netcat
nc 127.0.0.1 8080

# Via curl (pour une commande unique)
echo '{"command": "status"}' | nc 127.0.0.1 8080
```

### Commandes Disponibles

#### Gestion du Protocole
```json
// Vérifier l'état
{"command": "status"}

// Démarrer le protocole
{"command": "start"}

// Arrêter le protocole
{"command": "stop"}
```

#### Consultation des Voisins
```json
// Lister nos voisins directs
{"command": "neighbors"}

// Lister les voisins d'un routeur spécifique
{"command": "neighbors_of", "args": "R2"}
```

#### Table de Routage
```json
// Afficher la table de routage actuelle
{"command": "routing_table"}
```

#### Aide
```json
// Afficher l'aide
{"command": "help"}
```

## Configuration du Protocole

### Timeouts et Intervalles
- **Messages Hello** : 4 secondes
- **Mises à jour de routage** : 8 secondes
- **Timeout voisin** : 12 secondes (marqué comme mort)
- **Timeout route** : 16 secondes (supprimée)
- **Nettoyage** : 5 secondes

### Ports Réseau
- **Port d'écoute par défaut** : 5555 (protocole de routage)
- **Port de contrôle par défaut** : 8080 (interface d'administration)

## Dépannage

### Vérification des Logs
```bash
# Lancement avec logs détaillés
RUST_LOG=debug custom-ospf run --interfaces eth0 --sysname R1 --debug

# Filtrer les logs par module
RUST_LOG=custom_ospf::protocol=debug custom-ospf run ...
```

### Debug Avancé

#### Mode Debug Intégré
Utilisez le flag `--debug` pour afficher automatiquement l'état toutes les 30 secondes :
```
=== DEBUG STATUS ===
Neighbors (2): 
  R2 at 192.168.1.10:5555 - ALIVE (last seen: 2s ago)
  R3 at 192.168.1.20:5555 - ALIVE (last seen: 4s ago)
Current routing table:
  192.168.1.0/24 via direct dev eth0 metric 0 (Direct)
  10.0.0.0/24 via 192.168.1.10 dev eth0 metric 2 (Protocol)
===================
```


## Auteur

**Luca Ceccarelli** - luca.ceccarelli@etu.mines-ales.fr