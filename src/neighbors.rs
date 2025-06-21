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
    time::timeout,
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

    // 2) Channel pour communication interne avec buffer plus grand
    let (internal_tx, mut internal_rx) = mpsc::channel::<InternalMsg>(100);

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

    // 4) Task de réception avec meilleure gestion d'erreurs
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
                                // Utilisation de try_send pour éviter les blocages
                                match tx_clone.try_send(InternalMsg::HelloReceived {
                                    sysname: msg.sysname.clone(),
                                    ip: src.ip().to_string(),
                                }) {
                                    Ok(()) => println!("HELLO envoyé au channel"),
                                    Err(e) => println!("ERREUR envoi HELLO au channel: {}", e),
                                }
                            }
                            "LSA" => {
                                if let Some(neis) = msg.neighbors {
                                    println!("LSA traité de {} ({} voisins)", msg.sysname, neis.len());
                                    match tx_clone.try_send(InternalMsg::LsaReceived {
                                        sysname: msg.sysname.clone(),
                                        neighbors: neis,
                                    }) {
                                        Ok(()) => println!("LSA envoyé au channel"),
                                        Err(e) => println!("ERREUR envoi LSA au channel: {}", e),
                                    }
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
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    });

    // 5) Task de traitement des messages internes avec timeout
    let direct_processor = direct.clone();
    let lsa_processor = lsa.clone();

    tokio::spawn(async move {
        println!("PROCESSEUR démarré");
        loop {
            // Utilisation de timeout pour éviter les blocages
            match timeout(Duration::from_secs(1), internal_rx.recv()).await {
                Ok(Some(msg)) => {
                    println!("PROCESS: {:?}", msg);
                    match msg {
                        InternalMsg::HelloReceived { sysname, ip } => {
                            println!("PROCESS HELLO: {} -> {}", sysname, ip);
                            // Utilisation de try_write avec timeout
                            match timeout(Duration::from_millis(100), direct_processor.write()).await {
                                Ok(mut guard) => {
                                    guard.insert(sysname.clone(), ip);
                                    println!("VOISIN AJOUTE: {}", sysname);
                                }
                                Err(_) => println!("TIMEOUT lors de l'écriture direct pour {}", sysname),
                            }
                        }
                        InternalMsg::LsaReceived { sysname, neighbors } => {
                            println!("PROCESS LSA: {} avec {:?}", sysname, neighbors);
                            match timeout(Duration::from_millis(100), lsa_processor.write()).await {
                                Ok(mut guard) => {
                                    guard.insert(sysname.clone(), neighbors.clone());
                                    println!("LSA AJOUTE: {} -> {:?}", sysname, neighbors);
                                }
                                Err(_) => println!("TIMEOUT lors de l'écriture LSA pour {}", sysname),
                            }
                        }
                    }
                }
                Ok(None) => {
                    println!("Channel fermé, arrêt du processeur");
                    break;
                }
                Err(_) => {
                    // Timeout, on continue la boucle pour éviter les blocages
                    continue;
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

    // 7) Task d'émission avec meilleure gestion des erreurs
    let direct_emit = direct.clone();
    let sys_emit = sysname.clone();

    tokio::spawn(async move {
        let mcast_addr: SocketAddrV4 = format!("{}:{}", MCAST_ADDR, port).parse().unwrap();
        println!("EMISSION démarrée vers {}", mcast_addr);

        // Attendre un peu avant de commencer
        tokio::time::sleep(Duration::from_secs(2)).await;

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

            tokio::time::sleep(Duration::from_secs(5)).await;

            // LSA avec timeout pour éviter les blocages
            let neighbors = match timeout(Duration::from_millis(100), direct_emit.read()).await {
                Ok(guard) => guard.keys().cloned().collect::<Vec<_>>(),
                Err(_) => {
                    println!("TIMEOUT lecture voisins pour LSA");
                    continue;
                }
            };

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

            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });

    // 8) Task de debug avec timeouts
    let direct_debug = direct.clone();
    let lsa_debug = lsa.clone();
    let sys_debug = sysname.clone();

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;

        loop {
            println!("=== DEBUG {} ===", sys_debug);

            match timeout(Duration::from_millis(100), direct_debug.read()).await {
                Ok(direct_map) => {
                    println!("Voisins: {:?}", *direct_map);
                }
                Err(_) => println!("TIMEOUT lecture voisins debug"),
            }

            match timeout(Duration::from_millis(100), lsa_debug.read()).await {
                Ok(lsa_map) => {
                    println!("LSAs: {:?}", *lsa_map);
                }
                Err(_) => println!("TIMEOUT lecture LSAs debug"),
            }

            println!("=== FIN DEBUG ===");
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    });

    println!("=== {} PRET ===", sysname);
    Ok(Discovery { direct, lsa })
}