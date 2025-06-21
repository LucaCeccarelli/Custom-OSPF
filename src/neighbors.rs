use tokio::net::UdpSocket;
use serde::{Serialize, Deserialize};
use tokio::sync::RwLock;
use std::{sync::Arc, collections::HashMap};

#[derive(Serialize, Deserialize, Debug, Clone)]
struct LsaMsg {
    typ:       String,            // "HELLO" ou "LSA"
    sysname:   String,
    neighbors: Option<Vec<String>>,
}

/// Table de voisins directs (sysname → IP)
type DirNeighTable = Arc<RwLock<HashMap<String, String>>>;

/// Table LSA globale (sysname → liste de voisins)
pub type LsaTable = Arc<RwLock<HashMap<String, Vec<String>>>>;

pub struct Discovery {
    pub direct: DirNeighTable,
    pub lsa:    LsaTable,
}

/// Lance en tâche de fond :
//— un listener UDP (chaque iface) qui
//  • stocke dans `direct` les HELLO reçus (voisins directs)
//  • stocke dans `lsa` les LSAs reçus (graph global partiel)
//— un sender périodique qui :
//  • émet HELLO (pour dire “je suis”)
//  • puis, 2s plus tard, émet un LSA avec `direct.keys()`
pub async fn start_discovery(
    sysname: String,
    iface_ips: Vec<String>,
    port: u16,
) -> anyhow::Result<Discovery> {
    let direct = Arc::new(RwLock::new(HashMap::new()));
    let lsa    = Arc::new(RwLock::new(HashMap::new()));

    // Listener sur chaque interface
    for ip in &iface_ips {
        let sock = UdpSocket::bind(format!("{}:{}", ip, port)).await?;
        let direct_tab = direct.clone();
        let lsa_tab    = lsa.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            loop {
                if let Ok((len, src)) = sock.recv_from(&mut buf).await {
                    if let Ok(msg) = serde_json::from_slice::<LsaMsg>(&buf[..len]) {
                        match msg.typ.as_str() {
                            "HELLO" => {
                                direct_tab.write()
                                    .await
                                    .insert(msg.sysname.clone(), src.ip().to_string());
                            }
                            "LSA" => {
                                if let Some(neis) = msg.neighbors {
                                    lsa_tab.write()
                                        .await
                                        .insert(msg.sysname.clone(), neis);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        });
    }

    // Un seul socket d'émission pour HELLO+LSA
    let sock = UdpSocket::bind("0.0.0.0:0").await?;
    let direct_send = direct.clone();
    let sys = sysname.clone();
    tokio::spawn(async move {
        loop {
            // 1) HELLO
            let hello = LsaMsg { typ: "HELLO".into(), sysname: sys.clone(), neighbors: None };
            let data  = serde_json::to_vec(&hello).unwrap();
            for target in &iface_ips {
                let _ = sock.send_to(&data, format!("{}:{}", target, port)).await;
            }

            // 2s plus tard, annonce LSA
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let neis = direct_send
                .read().await
                .keys().cloned().collect::<Vec<_>>();
            let lsa_msg = LsaMsg { typ: "LSA".into(), sysname: sys.clone(), neighbors: Some(neis) };
            let data2   = serde_json::to_vec(&lsa_msg).unwrap();
            for target in &iface_ips {
                let _ = sock.send_to(&data2, format!("{}:{}", target, port)).await;
            }

            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });

    Ok(Discovery { direct, lsa })
}
