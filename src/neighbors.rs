use anyhow::Result;
use get_if_addrs::{get_if_addrs, IfAddr};
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddrV4},
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::UdpSocket,
    sync::RwLock,
    time::timeout,
};

#[derive(Serialize, Deserialize, Debug, Clone)]
struct LsaMsg {
    typ:       String,
    sysname:   String,
    neighbors: Option<Vec<String>>,
}

type DirNeighTable = Arc<RwLock<HashMap<String, String>>>;
pub type LsaTable  = Arc<RwLock<HashMap<String, Vec<String>>>>;

pub struct Discovery {
    pub direct: DirNeighTable,
    pub lsa:    LsaTable,
}

const MCAST_ADDR: &str = "224.0.0.5";

pub async fn start_discovery(
    sysname:     String,
    iface_names: Vec<String>,
    port:        u16,
) -> Result<Discovery> {
    let direct = Arc::new(RwLock::new(HashMap::new()));
    let lsa    = Arc::new(RwLock::new(HashMap::new()));

    println!("=== Démarrage discovery pour {} sur interfaces {:?} ===", sysname, iface_names);

    // 1) Récupère les IPv4 des interfaces
    let mut ifs = Vec::new();
    let all_ifs = get_if_addrs()?;
    println!("Interfaces disponibles:");
    for iface in &all_ifs {
        println!("  - {} : {:?}", iface.name, iface.addr);
    }

    for iface in all_ifs {
        if let IfAddr::V4(v4) = iface.addr {
            if iface_names.contains(&iface.name) {
                println!("Interface sélectionnée: {} -> {}", iface.name, v4.ip);
                ifs.push((iface.name.clone(), v4.ip));
            }
        }
    }

    if ifs.is_empty() {
        anyhow::bail!("Aucune interface IPv4 trouvée dans {:?}", iface_names);
    }

    // 2) Socket de réception PARTAGÉE
    println!("=== Configuration socket réception partagée sur port {} ===", port);
    let std_sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;

    // CRUCIAL: Permettre le partage du port
    std_sock.set_reuse_address(true)?;
    #[cfg(unix)]
    std_sock.set_reuse_port(true)?;

    // Bind sur INADDR_ANY pour recevoir de partout
    std_sock.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port).into())?;

    // Join le multicast sur TOUTES nos interfaces
    for (_name, local_ip) in &ifs {
        println!("Join multicast 224.0.0.5 sur interface {}", local_ip);
        match std_sock.join_multicast_v4(&MCAST_ADDR.parse()?, local_ip) {
            Ok(_) => println!("  ✓ Joint avec succès sur {}", local_ip),
            Err(e) => println!("  ✗ Erreur join sur {}: {}", local_ip, e),
        }
    }

    let recv_sock = UdpSocket::from_std(std_sock.into())?;

    // 3) Task de réception avec debug détaillé
    let direct_cl = direct.clone();
    let lsa_cl = lsa.clone();
    let sysname_for_recv = sysname.clone();

    tokio::spawn(async move {
        println!("=== Task réception démarrée pour {} ===", sysname_for_recv);
        let mut buf = [0u8; 2048];
        let mut packet_count = 0;

        loop {
            match recv_sock.recv_from(&mut buf).await {
                Ok((len, src)) => {
                    packet_count += 1;
                    println!("📦 PAQUET #{} de {} ({} bytes)", packet_count, src, len);

                    if let Ok(msg) = serde_json::from_slice::<LsaMsg>(&buf[..len]) {
                        println!("📋 Message: type={}, sysname={}", msg.typ, msg.sysname);

                        // Ne pas traiter nos propres messages
                        if msg.sysname == sysname_for_recv {
                            println!("🚫 Ignorer notre propre message de {}", msg.sysname);
                            continue;
                        }

                        println!("✅ Message externe de {} ({}): {}", msg.sysname, src.ip(), msg.typ);

                        match msg.typ.as_str() {
                            "HELLO" => {
                                println!("👋 Traitement HELLO de {} ({})", msg.sysname, src.ip());

                                match timeout(Duration::from_millis(500), direct_cl.write()).await {
                                    Ok(mut direct_map) => {
                                        direct_map.insert(
                                            msg.sysname.clone(),
                                            src.ip().to_string(),
                                        );
                                        println!("✅ Voisin {} ajouté (IP: {})", msg.sysname, src.ip());
                                    }
                                    Err(_) => {
                                        println!("⏰ Timeout écriture direct_cl pour {}", msg.sysname);
                                    }
                                }
                            }
                            "LSA" => {
                                println!("📊 Traitement LSA de {}", msg.sysname);
                                if let Some(neis) = &msg.neighbors {
                                    match timeout(Duration::from_millis(500), lsa_cl.write()).await {
                                        Ok(mut lsa_map) => {
                                            lsa_map.insert(msg.sysname.clone(), neis.clone());
                                            println!("✅ LSA de {} mis à jour avec {} voisins: {:?}",
                                                     msg.sysname, neis.len(), neis);
                                        }
                                        Err(_) => {
                                            println!("⏰ Timeout écriture lsa_cl pour {}", msg.sysname);
                                        }
                                    }
                                }
                            }
                            _ => {
                                println!("❓ Type message inconnu: {}", msg.typ);
                            }
                        }
                    } else {
                        println!("❌ Erreur décodage JSON de {}", src);
                    }
                }
                Err(e) => {
                    println!("💥 Erreur réception: {}", e);
                }
            }
        }
    });

    // 4) Sockets d'émission (une par interface)
    let mut send_socks = Vec::new();
    for (name, local_ip) in &ifs {
        println!("=== Configuration socket émission sur {} ({}) ===", name, local_ip);

        let std_sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        std_sock.set_reuse_address(true)?;
        #[cfg(unix)]
        std_sock.set_reuse_port(true)?;

        // Bind sur l'IP locale (pas sur 0.0.0.0)
        std_sock.bind(&SocketAddrV4::new(*local_ip, 0).into())?;

        // Configurer l'interface multicast sortante
        std_sock.set_multicast_if_v4(local_ip)?;
        std_sock.set_multicast_ttl_v4(2)?; // TTL=2 pour traverser un switch

        let sock = UdpSocket::from_std(std_sock.into())?;
        send_socks.push((name.clone(), sock));
        println!("✅ Socket émission OK sur {} ({})", name, local_ip);
    }

    // 5) Task d'émission périodique
    let direct_for_emit = direct.clone();
    let sys_for_emit = sysname.clone();

    tokio::spawn(async move {
        let mcast_addr: SocketAddrV4 = format!("{}:{}", MCAST_ADDR, port).parse().unwrap();
        println!("=== Task émission démarrée vers {} ===", mcast_addr);

        // Attendre 2 secondes avant de commencer
        tokio::time::sleep(Duration::from_secs(2)).await;

        loop {
            // HELLO
            let hello = LsaMsg {
                typ:       "HELLO".to_string(),
                sysname:   sys_for_emit.clone(),
                neighbors: None,
            };

            match serde_json::to_vec(&hello) {
                Ok(data) => {
                    println!("📤 Envoi HELLO de {} ({} bytes)", sys_for_emit, data.len());

                    for (iface_name, sock) in &send_socks {
                        match sock.send_to(&data, mcast_addr).await {
                            Ok(n) => println!("  ✅ HELLO envoyé sur {} ({} bytes)", iface_name, n),
                            Err(e) => println!("  ❌ Erreur HELLO sur {}: {}", iface_name, e),
                        }
                    }
                }
                Err(e) => println!("❌ Erreur sérialisation HELLO: {}", e),
            }

            // Attendre 2 secondes puis envoyer LSA
            tokio::time::sleep(Duration::from_secs(2)).await;

            let neighbors = match timeout(Duration::from_millis(500), direct_for_emit.read()).await {
                Ok(direct_map) => direct_map.keys().cloned().collect::<Vec<_>>(),
                Err(_) => {
                    println!("⏰ Timeout lecture neighbors pour LSA");
                    Vec::new()
                }
            };

            let lsa_msg = LsaMsg {
                typ:       "LSA".to_string(),
                sysname:   sys_for_emit.clone(),
                neighbors: Some(neighbors.clone()),
            };

            match serde_json::to_vec(&lsa_msg) {
                Ok(data) => {
                    println!("📤 Envoi LSA de {} avec {} voisins: {:?}",
                             sys_for_emit, neighbors.len(), neighbors);

                    for (iface_name, sock) in &send_socks {
                        match sock.send_to(&data, mcast_addr).await {
                            Ok(n) => println!("  ✅ LSA envoyé sur {} ({} bytes)", iface_name, n),
                            Err(e) => println!("  ❌ Erreur LSA sur {}: {}", iface_name, e),
                        }
                    }
                }
                Err(e) => println!("❌ Erreur sérialisation LSA: {}", e),
            }

            // Cycle de 10 secondes total (2s HELLO + 2s LSA + 6s pause)
            tokio::time::sleep(Duration::from_secs(6)).await;
        }
    });

    // 6) Task de debug périodique
    let direct_debug = direct.clone();
    let lsa_debug = lsa.clone();
    let sys_debug = sysname.clone();

    tokio::spawn(async move {
        // Premier debug après 8 secondes
        tokio::time::sleep(Duration::from_secs(8)).await;

        loop {
            println!("=== 🔍 DEBUG {} ===", sys_debug);

            match timeout(Duration::from_millis(500), direct_debug.read()).await {
                Ok(direct_map) => {
                    if direct_map.is_empty() {
                        println!("👥 Aucun voisin direct");
                    } else {
                        println!("👥 Voisins directs ({}):", direct_map.len());
                        for (name, ip) in direct_map.iter() {
                            println!("  - {} -> {}", name, ip);
                        }
                    }
                }
                Err(_) => {
                    println!("⏰ Timeout lecture direct_debug");
                }
            }

            match timeout(Duration::from_millis(500), lsa_debug.read()).await {
                Ok(lsa_map) => {
                    if lsa_map.is_empty() {
                        println!("📊 Aucune LSA reçue");
                    } else {
                        println!("📊 LSA reçues ({}):", lsa_map.len());
                        for (name, neighbors) in lsa_map.iter() {
                            println!("  - {} a {} voisins: {:?}", name, neighbors.len(), neighbors);
                        }
                    }
                }
                Err(_) => {
                    println!("⏰ Timeout lecture lsa_debug");
                }
            }

            println!("=== 🔍 FIN DEBUG ===\n");

            // Debug toutes les 15 secondes
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    });

    println!("✅ Discovery initialisé pour {}", sysname);
    Ok(Discovery { direct, lsa })
}
