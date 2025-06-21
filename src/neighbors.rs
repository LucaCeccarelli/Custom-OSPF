use anyhow::Result;
use get_if_addrs::{get_if_addrs, IfAddr};
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddrV4},
    sync::Arc,
};
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
) -> Result<Discovery> {
    let direct = Arc::new(RwLock::new(HashMap::new()));
    let lsa    = Arc::new(RwLock::new(HashMap::new()));

    println!("Démarrage discovery pour {} sur interfaces {:?}", sysname, iface_names);

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

    // 2) Socket de réception par interface
    for (_name, local_ip) in &ifs {
        println!("Configuration socket réception sur {}:{}", local_ip, port);

        let std_sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        std_sock.set_reuse_address(true)?;
        #[cfg(unix)] std_sock.set_reuse_port(true)?;
        std_sock.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port).into())?;
        std_sock.join_multicast_v4(&MCAST_ADDR.parse()?, local_ip)?;
        let sock = UdpSocket::from_std(std_sock.into())?;

        let direct_cl = direct.clone();
        let lsa_cl    = lsa.clone();
        let iface_ip = *local_ip;

        tokio::spawn(async move {
            println!("Task de réception démarrée pour {}", iface_ip);
            let mut buf = [0u8; 2048];
            loop {
                match sock.recv_from(&mut buf).await {
                    Ok((len, src)) => {
                        println!("Paquet reçu de {} ({} bytes)", src, len);
                        if let Ok(msg) = serde_json::from_slice::<LsaMsg>(&buf[..len]) {
                            println!("Message décodé: {:?}", msg);
                            match msg.typ.as_str() {
                                "HELLO" => {
                                    println!("HELLO reçu de {}", msg.sysname);
                                    direct_cl.write().await.insert(
                                        msg.sysname.clone(),
                                        src.ip().to_string(),
                                    );
                                }
                                "LSA" => {
                                    println!("LSA reçu de {}", msg.sysname);
                                    if let Some(neis) = msg.neighbors {
                                        lsa_cl.write().await.insert(
                                            msg.sysname.clone(),
                                            neis,
                                        );
                                    }
                                }
                                _ => {}
                            }
                        } else {
                            println!("Erreur décodage message");
                        }
                    }
                    Err(e) => {
                        println!("Erreur réception: {}", e);
                    }
                }
            }
        });
    }

    // 3) Socket d'émission par interface
    let mut send_socks = Vec::with_capacity(ifs.len());
    for (_name, local_ip) in &ifs {
        println!("Configuration socket émission sur {}", local_ip);

        let std_sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        std_sock.set_reuse_address(true)?;
        #[cfg(unix)] std_sock.set_reuse_port(true)?;
        std_sock.bind(&SocketAddrV4::new(*local_ip, 0).into())?;
        std_sock.set_multicast_if_v4(local_ip)?;
        let sock = UdpSocket::from_std(std_sock.into())?;
        send_socks.push(sock);
    }

    // 4) Task d'émission périodique
    let direct_for_emit = direct.clone();
    let sys = sysname.clone();
    tokio::spawn(async move {
        let mcast_addr: SocketAddrV4 =
            format!("{}:{}", MCAST_ADDR, port).parse().unwrap();

        println!("Task d'émission démarrée vers {}", mcast_addr);

        loop {
            // HELLO
            let hello = LsaMsg {
                typ:       "HELLO".into(),
                sysname:   sys.clone(),
                neighbors: None,
            };
            let data = serde_json::to_vec(&hello).unwrap();
            println!("Envoi HELLO de {}", sys);
            for sock in &send_socks {
                match sock.send_to(&data, mcast_addr).await {
                    Ok(n) => println!("HELLO envoyé ({} bytes)", n),
                    Err(e) => println!("Erreur envoi HELLO: {}", e),
                }
            }

            // 2s plus tard, LSA
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let neis = direct_for_emit.read().await.keys().cloned().collect::<Vec<_>>();
            let lsa_msg = LsaMsg {
                typ:       "LSA".into(),
                sysname:   sys.clone(),
                neighbors: Some(neis.clone()),
            };
            let data2 = serde_json::to_vec(&lsa_msg).unwrap();
            println!("Envoi LSA de {} avec voisins: {:?}", sys, neis);
            for sock in &send_socks {
                match sock.send_to(&data2, mcast_addr).await {
                    Ok(n) => println!("LSA envoyé ({} bytes)", n),
                    Err(e) => println!("Erreur envoi LSA: {}", e),
                }
            }

            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });

    Ok(Discovery { direct, lsa })
}
