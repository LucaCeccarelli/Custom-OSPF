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
use tokio::sync::watch;
use tokio::{
    net::UdpSocket,
    sync::RwLock,
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
    notifier:    watch::Sender<()>,
) -> Result<Discovery> {
    let direct = Arc::new(RwLock::new(HashMap::new()));
    let lsa    = Arc::new(RwLock::new(HashMap::new()));

    println!("=== DEBUT {} ===", sysname);

    // 1) Récupérer IPv4 des interfaces demandées
    let mut ifs = Vec::new();
    for iface in get_if_addrs()? {
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

    // 2) Socket réception UDP multicast
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

    // 3) Task réception
    {
        let sysname_recv = sysname.clone();
        let direct_recv = direct.clone();
        let lsa_recv = lsa.clone();
        let notifier_recv = notifier.clone();

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
                                    let mut guard = direct_recv.write().await;
                                    guard.insert(msg.sysname.clone(), src.ip().to_string());
                                    println!("VOISIN AJOUTE DIRECTEMENT: {} -> {}", msg.sysname, src.ip());
                                }
                                "LSA" => {
                                    if let Some(neis) = msg.neighbors {
                                        println!("LSA traité de {} ({} voisins)", msg.sysname, neis.len());
                                        let mut is_new_data = false;
                                        {
                                            let mut guard = lsa_recv.write().await;
                                            if guard.get(&msg.sysname) != Some(&neis) {
                                                guard.insert(msg.sysname.clone(), neis.clone());
                                                is_new_data = true;
                                                println!("LSA AJOUTE/MIS A JOUR: {} -> {:?}", msg.sysname, neis);
                                            }
                                        }
                                        if is_new_data {
                                            let _ = notifier_recv.send(());
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
    }

    // 4) Sockets émission
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

    // 5) Task émission périodique HELLO + LSA
    {
        let direct_emit = direct.clone();
        let sys_emit = sysname.clone();

        tokio::spawn(async move {
            let mcast_addr: SocketAddrV4 = format!("{}:{}", MCAST_ADDR, port).parse().unwrap();
            println!("EMISSION démarrée vers {}", mcast_addr);

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
                            Err(e) => println!("HELLO ERR socket {}: {}", i, e),
                        }
                    }
                }

                // LSA
                let neis = {
                    let guard = direct_emit.read().await;
                    guard.keys().cloned().collect::<Vec<_>>()
                };

                let lsa_msg = LsaMsg {
                    typ: "LSA".to_string(),
                    sysname: sys_emit.clone(),
                    neighbors: Some(neis),
                };

                if let Ok(data) = serde_json::to_vec(&lsa_msg) {
                    println!("ENVOI LSA de {}", sys_emit);
                    for (i, sock) in send_socks.iter().enumerate() {
                        match sock.send_to(&data, mcast_addr).await {
                            Ok(_) => println!("LSA OK sur socket {}", i),
                            Err(e) => println!("LSA ERR socket {}: {}", i, e),
                        }
                    }
                }

                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        });
    }

    println!("=== {} PRET ===", sysname);

    Ok(Discovery { direct, lsa })
}
