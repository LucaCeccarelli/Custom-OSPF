use crate::types::*;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::{Duration, SystemTime};
use tokio::time;

pub struct NetworkManager {
    socket: UdpSocket,
    local_ip: IpAddr,
    hostname: String,
    enabled_interfaces: Vec<String>,
    sequence_number: u32,
}

impl NetworkManager {
    pub fn new(bind_addr: SocketAddr, hostname: String) -> Result<Self, Box<dyn std::error::Error>> {
        let socket = UdpSocket::bind(bind_addr)?;
        socket.set_broadcast(true)?;

        Ok(NetworkManager {
            socket,
            local_ip: bind_addr.ip(),
            hostname,
            enabled_interfaces: Vec::new(),
            sequence_number: 0,
        })
    }

    pub fn add_interface(&mut self, interface: String) {
        self.enabled_interfaces.push(interface);
    }

    pub fn remove_interface(&mut self, interface: &str) {
        self.enabled_interfaces.retain(|i| i != interface);
    }

    pub async fn send_routing_update(&mut self, routes: Vec<Route>, neighbors: Vec<Router>) -> Result<(), Box<dyn std::error::Error>> {
        self.sequence_number += 1;

        let message = RoutingMessage {
            sender: self.local_ip,
            sequence: self.sequence_number,
            timestamp: SystemTime::now(),
            routes,
            neighbors,
        };

        let data = serde_json::to_vec(&message)?;

        // Broadcast to multicast address
        let multicast_addr: SocketAddr = "224.0.0.100:5000".parse().unwrap();
        self.socket.send_to(&data, multicast_addr)?;

        Ok(())
    }

    pub async fn receive_routing_update(&self) -> Result<RoutingMessage, Box<dyn std::error::Error>> {
        let mut buffer = [0; 65536];
        let (size, _) = self.socket.recv_from(&mut buffer)?;

        let message: RoutingMessage = serde_json::from_slice(&buffer[..size])?;
        Ok(message)
    }
}
