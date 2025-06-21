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
    sync::{mpsc, RwLock},
};

#[derive(Serialize, Deserialize, Debug, Clone)]
struct LsaMsg {
    typ:       String,
    sysname:   String,
    neighbors: Option<Vec<String>>,
}

// Messages internes pour éviter les deadlocks
#[derive(Debug, Clone)]
enum InternalMsg {
    HelloReceived { sysname: String, ip: String },
    LsaReceived { sysname: String, neighbors: Vec<String> },
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

    println!("=== DEBUT {} ===", sysname);

    // 1) Récupère les IPv4 des interfaces
    let mut ifs = Vec::new();
    let all_ifs = get_if_addrs()?;
    for iface in all_ifs {
        if let IfAddr::V4(v4) = iface.addr {
            if iface_names.contains(&iface.name) {
                println!("Interface: {} -> {}", iface.name, v4.ip);
                ifs.push((iface.name.clone(), v4.ip));
            }
        }
    }

    if ifs.is_empty() {
        anyhow::bail!("Aucune interface IPv4 trouvée dans {:?}", iface_names);
    }

    // 2) Channel pour communication interne (éviter les deadlocks)
    let (internal_tx, mut internal_rx) = mpsc::unbounded_channel::<InternalMsg>();

    // 3) Socket de réception
    println!("Configuration socket réception...");
    let std_sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    std_sock.set_reuse_address(true)?;
    #[cfg(unix)] std_sock.set_reuse_port(true)?;
    std_sock.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port).into())?;

    for (_name, local_ip) in &ifs {
        std_sock.join_multicast_v4(&MCAST_ADDR.parse()?, local_ip)?;
        println!("Joint multicast sur {}", local_ip);
    }

    let recv_sock = UdpSocket::from_std(std_sock.into())?;

    // 4) Task de réception (envoie vers channel, pas de RwLock)
    let sysname_recv = sysname.clone();
    let tx_clone = internal_tx.clone();

    tokio::spawn(async move {
        println!("RECEPTION démarrée pour {}", sysname_recv);
        let mut buf = [0u8; 2048];
        let mut count = 0;

        loop {
            match recv_sock.recv_from(&mut buf).await {
                Ok((len, src)) => {
                    count += 1;
                    println!("PAQUET #{} de {} ({} bytes)", count, src, len);

                    if let Ok(msg) = serde_json::from_slice::<LsaMsg>(&buf[..len]) {
                        println!("MSG: {} de {}", msg.typ, msg.sysname);

                        if msg.sysname == sysname_recv {
                            println!("IGNORE notre message");
                            continue;
                        }

                        println!("TRAITE message de {}", msg.sysname);

                        match msg.typ.as_str() {
                            "HELLO" => {
                                println!("HELLO traité de {}", msg.sysname);
                                let _ = tx_clone.send(InternalMsg::HelloReceived {
                                    sysname: msg.sysname.clone(),
                                    ip: src.ip().to_string(),
                                });
                                println!("HELLO envoyé au channel");
                            }
                            "LSA" => {
                                if let Some(neis) = msg.neighbors {
                                    println!("LSA traité de {} ({} voisins)", msg.sysname, neis.len());
                                    let _ = tx_clone.send(InternalMsg::LsaReceived {
                                        sysname: msg.sysname.clone(),
                                        neighbors: neis,
                                    });
                                    println!("LSA envoyé au channel");
                                }
                            }
                            _ => {}
                        }
                    } else {
                        println!("ERREUR decode de {}", src);
                    }
                }
                Err(e) => {
                    println!("ERREUR recv: {}", e);
                }
            }
        }
    });

    // 5) Task de traitement des messages internes (seule à modifier les RwLock)
    let direct_processor = direct.clone();
    let lsa_processor = lsa.clone();

    tokio::spawn(async move {
        println!("PROCESSEUR démarré");
        while let Some(msg) = internal_rx.recv().await {
            println!("PROCESS: {:?}", msg);
            match msg {
                InternalMsg::HelloReceived { sysname, ip } => {
                    println!("PROCESS HELLO: {} -> {}", sysname, ip);
                    direct_processor.write().await.insert(sysname.clone(), ip);
                    println!("VOISIN AJOUTE: {}", sysname);
                }
                InternalMsg::LsaReceived { sysname, neighbors } => {
                    println!("PROCESS LSA: {} avec {:?}", sysname, neighbors);
                    lsa_processor.write().await.insert(sysname.clone(), neighbors.clone());
                    println!("LSA AJOUTE: {} -> {:?}", sysname, neighbors);
                }
            }
        }
    });

    // 6) Sockets d'émission
    let mut send_socks = Vec::new();
    for (_name, local_ip) in &ifs {
        let std_sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        std_sock.set_reuse_address(true)?;
        #[cfg(unix)] std_sock.set_reuse_port(true)?;
        std_sock.bind(&SocketAddrV4::new(*local_ip, 0).into())?;
        std_sock.set_multicast_if_v4(local_ip)?;
        std_sock.set_multicast_ttl_v4(2)?;
        let sock = UdpSocket::from_std(std_sock.into())?;
        send_socks.push(sock);
    }

    // 7) Task d'émission
    let direct_emit = direct.clone();
    let sys_emit = sysname.clone();

    tokio::spawn(async move {
        let mcast_addr: SocketAddrV4 = format!("{}:{}", MCAST_ADDR, port).parse().unwrap();
        println!("EMISSION démarrée vers {}", mcast_addr);

        // Attendre un peu avant de commencer
        tokio::time::sleep(Duration::from_secs(1)).await;

        loop {
            // HELLO
            let hello = LsaMsg {
                typ: "HELLO".to_string(),
                sysname: sys_emit.clone(),
                neighbors: None,
            };

            if let Ok(data) = serde_json::to_vec(&hello) {
                println!("ENVOI HELLO de {}", sys_emit);
                for (i, sock) in send_socks.iter().enumerate() {
                    match sock.send_to(&data, mcast_addr).await {
                        Ok(_) => println!("HELLO OK sur socket {}", i),
                        Err(e) => println!("HELLO ERR sur socket {}: {}", i, e),
                    }
                }
            }

            tokio::time::sleep(Duration::from_secs(3)).await;

            // LSA
            let neighbors = direct_emit.read().await.keys().cloned().collect::<Vec<_>>();
            let lsa_msg = LsaMsg {
                typ: "LSA".to_string(),
                sysname: sys_emit.clone(),
                neighbors: Some(neighbors.clone()),
            };

            if let Ok(data) = serde_json::to_vec(&lsa_msg) {
                println!("ENVOI LSA de {} avec {:?}", sys_emit, neighbors);
                for (i, sock) in send_socks.iter().enumerate() {
                    match sock.send_to(&data, mcast_addr).await {
                        Ok(_) => println!("LSA OK sur socket {}", i),
                        Err(e) => println!("LSA ERR sur socket {}: {}", i, e),
                    }
                }
            }

            tokio::time::sleep(Duration::from_secs(7)).await;
        }
    });

    // 8) Task de debug simple
    let direct_debug = direct.clone();
    let lsa_debug = lsa.clone();
    let sys_debug = sysname.clone();

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;

        loop {
            println!("=== DEBUG {} ===", sys_debug);
            let direct_map = direct_debug.read().await;
            println!("Voisins: {:?}", *direct_map);
            drop(direct_map);

            let lsa_map = lsa_debug.read().await;
            println!("LSAs: {:?}", *lsa_map);
            drop(lsa_map);

            println!("=== FIN DEBUG ===");
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });

    println!("=== {} PRET ===", sysname);
    Ok(Discovery { direct, lsa })
}
