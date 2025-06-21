README d’utilisation du projet rust_router
========================================

rust_router est un démon de découverte de voisins et de calcul de routes IPv4 « à la RIP/OSPF » en Rust.

Prérequis
---------
- Linux avec Rust (édition 2021)
- `CAP_NET_ADMIN` ou exécution en root pour modifier la table de routage
- Cargo, tokio, petgraph, rtnetlink installés via `cargo build`

Installation
------------
1. Cloner le dépôt
   ```bash
   git clone https://…/rust_router.git
   cd rust_router
   ```
2. Compiler en release
   ```bash
   cargo build --release
   ```
3. Le binaire se trouve dans `target/release/rust_router`

Command-Line Interface
----------------------

Tout s’appelle via le binaire `rust_router` :

rust_router --help  
rust_router <commande> --help

Commandes principales
---------------------

1) run  
   Lancer le démon : découverte de voisins, échange de LSAs, calcul et injection de routes.

   Arguments obligatoires :
   • `--interfaces <IP>` (multiple) : liste des IP locales à écouter/émettre  
   • `--sysname <NAME>` : identifiant unique de ce routeur

   Options :
   • `--hello-port <PORT>` (défaut 5000)

   Exemple :
   ```bash
   sudo target/release/rust_router \
     run \
      --interfaces eth0 \
      --interfaces br0 \
      --hello-port 5000 \
      --sysname R1 \
      --hello-port 5000
   ```

2) enable  
   Active localement le protocole (autorise l’installation de routes).
   ```bash
   ./rust_router enable
   ```

3) disable  
   Désactive localement le protocole (arrête l’installation de routes).
   ```bash
   ./rust_router disable
   ```

4) show-status  
   Affiche si le protocole est ON ou OFF.
   ```bash
   ./rust_router show-status
   ```

Flux de fonctionnement
----------------------

- Au démarrage (`run`), le programme écoute des messages HELLO/LSA sur chaque interface.
- Il émet périodiquement (5 s) :
    1. un HELLO pour annoncer sa présence
    2. un LSA contenant la liste de ses voisins directs
- Il agrège toutes les LSAs reçues pour reconstruire le graphe global,
- Il calcule ensuite les plus courts chemins entre chaque paire de nœuds,
- Il installe les routes IPv4 via Netlink si le protocole est `enable`.

Notes
-----
- Pour tester sur plusieurs « routeurs » virtuels, lancer plusieurs instances sur des VMs ou des conteneurs, chacune avec son `--sysname` et ses interfaces.
- Les interfaces passées en CLI doivent être celles sur lesquelles on veut échanger HELLO/LSA (ne pas oublier routage IP ou ponts adaptés).
- Le démontre n’installe que les routes dynamiques issues du calcul ; la suppression automatique ou la gestion des liens down/up est à compléter.

Bonne expérimentation !