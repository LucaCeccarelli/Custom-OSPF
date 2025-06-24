Commandes principales
---------------------

1) run  
   Lancer le démon : découverte de voisins, échange de Messages Hello, calcul et injection de routes.

   Arguments obligatoires :
   • `--interfaces <ETH>` (multiple) : liste des interfaces réseau qui seront utilisées par le protocole  
   • `--sysname <NAME>` : identifiant unique de ce routeur

   Options :
   • `--listen-port <PORT>` (défaut 5000)
2) • `--debug`

   Exemple :
   ```bash
   custom-ospf \
     run \
      --interfaces eth0 \
      --interfaces br0 \
      --sysname R1 \
      --control-port 8080 \
      --debug
   ```

## Commandes de controle
```bash
# Connect to control server
telnet 127.0.0.1 8080

# Get all neighbors
{"command": "neighbors"}

# Get neighbors of specific router
{"command": "neighbors_of", "args": "R2"}

# Stop/start protocol
{"command": "stop"}
{"command": "start"}

# Check status
{"command": "status"}

# View routing table
{"command": "routing_table"}
```