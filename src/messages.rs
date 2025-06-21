use crate::neighbors::NeighborManager;
use crate::network::NetworkGraph;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    net::{SocketAddr, UdpSocket},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Message {
    Hello { sysname: String, ip: String },
    LSA { sysname: String, neighbors: Vec<String> },
}

pub fn send_message(socket: &UdpSocket, dest: &str, msg: &Message) {
    if let Ok(data) = serde_json::to_vec(msg) {
        let _ = socket.send_to(&data, dest);
    }
}

pub fn start_sockets(
    _ifaces: &HashMap<String, String>,
    _sysname: String,
) -> (UdpSocket, Sender<Message>, Receiver<Message>) {
    let socket = UdpSocket::bind("0.0.0.0:8888").expect("Could not bind to port 8888");
    socket
        .set_multicast_loop_v4(true)
        .expect("Could not enable multicast loop");

    socket
        .join_multicast_v4(
            &"224.0.0.1".parse().unwrap(),
            &"0.0.0.0".parse().unwrap(),
        )
        .expect("Could not join multicast group");

    let (tx, rx) = mpsc::channel();
    (socket, tx, rx)
}

pub fn spawn_sender(socket: UdpSocket, ifaces: HashMap<String, String>, sysname: String) {
    thread::spawn(move || loop {
        for (_iface, ip) in &ifaces {
            let msg = Message::Hello {
                sysname: sysname.clone(),
                ip: ip.clone(),
            };
            send_message(&socket, "224.0.0.1:8888", &msg);
        }
        thread::sleep(Duration::from_secs(5));
    });
}

pub fn spawn_receiver(
    socket: UdpSocket,
    ifaces: &HashMap<String, String>,
    sysname: String,
    tx: Sender<Message>,
) {
    let ifaces_cloned = ifaces.clone();

    thread::spawn(move || loop {
        let mut buf = [0u8; 512];
        if let Ok((n, src)) = socket.recv_from(&mut buf) {
            if let Ok(msg) = serde_json::from_slice::<Message>(&buf[..n]) {
                match &msg {
                    Message::Hello { sysname: sender_name, ip } => {
                        if src.ip().to_string() != *ip {
                            println!(
                                "[WARN] Adresse source {} ne correspond pas à l'IP déclarée {}",
                                src.ip(),
                                ip
                            );
                        } else if sender_name != &sysname {
                            println!(
                                "[INFO] Reçu HELLO de {} ({})",
                                sender_name, ip
                            );
                        }
                    }
                    Message::LSA { sysname: sender_name, neighbors } => {
                        println!(
                            "[INFO] Reçu LSA de {}: {:?}",
                            sender_name, neighbors
                        );
                    }
                }

                let _ = tx.send(msg);
            }
        }
    });
}
