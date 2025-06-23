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
      --debug
   ```
