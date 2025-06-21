use std::collections::HashMap;
use pnet::datalink::{self, NetworkInterface};

pub fn get_interface_ips(names: &[String]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let interfaces = datalink::interfaces();

    for name in names {
        if let Some(interface) = interfaces.iter().find(|iface| &iface.name == name) {
            if let Some(ip) = interface.ips.iter().find(|ip| ip.is_ipv4()) {
                map.insert(name.clone(), ip.ip().to_string());
                println!("Interface: {} -> {}", name, ip.ip());
            }
        }
    }

    map
}
