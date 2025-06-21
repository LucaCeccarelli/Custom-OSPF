use std::collections::HashMap;
use std::net::{UdpSocket, SocketAddrV4};
use std::sync::mpsc::{Sender, Receiver};
use std::thread;
use std::time::Duration;
use serde::{Serialize, Deserialize};

use crate::neighbors::NeighborManager;
use crate::network::NetworkGraph;

// Exemple minimal du type Message (à adapter selon ton code)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    Hello(String),
    Lsa(String),
}

// Fonction d’envoi simple d’un message (JSON ou autre)
pub fn send_message(socket: &UdpSocket, dest: &str, msg: &Message) -> std::io::Result<()> {
    let data = serde_json::to_vec(msg).unwrap(); // besoin de serde_json dans Cargo.toml
    socket.send_to(&data, dest)?;
    Ok(())
}

// Fonction qui lance le thread d’envoi périodique des messages HELLO
pub fn spawn_sender(ifaces: HashMap<String, String>, sysname: String) {
    thread::spawn(move || {
        let socket = UdpSocket::bind("0.0.0.0:0").expect("Erreur bind sender");
        loop {
            for (iface, ip) in &ifaces {
                let hello_msg = Message::Hello(sysname.clone());
                let multicast_addr = "224.0.0.1:8888";
                if let Err(e) = send_message(&socket, multicast_addr, &hello_msg) {
                    eprintln!("[ERROR] send_message: {:?}", e);
                }
                println!("[INFO] Envoi HELLO depuis {} ({})", iface, ip);
            }
            thread::sleep(Duration::from_secs(5));
        }
    });
}

// Fonction qui lance le thread de réception des messages UDP multicast
pub fn spawn_receiver(
    socket: UdpSocket,
    ifaces: &HashMap<String, String>,
    sysname: String,
    tx: Sender<Message>,
) {
    let ifaces = ifaces.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 1500];
        loop {
            match socket.recv_from(&mut buf) {
                Ok((size, src_addr)) => {
                    let data = &buf[..size];
                    if let Ok(msg) = serde_json::from_slice::<Message>(data) {
                        // Vérifie que l’adresse source est dans ifaces, sinon warning
                        let src_ip = src_addr.ip().to_string();
                        if !ifaces.values().any(|ip| *ip == src_ip) {
                            println!("[WARN] Adresse source {} ne correspond pas à l'IP déclarée", src_ip);
                        }
                        println!("[RECV] Message reçu de {}: {:?}", src_ip, msg);
                        // Envoie le message reçu au thread principal via le channel
                        if tx.send(msg).is_err() {
                            eprintln!("[ERROR] Échec envoi message au thread principal");
                        }
                    } else {
                        eprintln!("[ERROR] Impossible de décoder le message reçu");
                    }
                }
                Err(e) => {
                    eprintln!("[ERROR] Erreur réception socket: {:?}", e);
                    break;
                }
            }
        }
    });
}
