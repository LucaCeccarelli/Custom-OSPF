use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use crate::{RouterId, NetworkId};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Neighbor {
    pub router_id: RouterId,
    pub router_name: String,
    pub ip_address: IpAddr,
    pub network: NetworkId,
    pub bandwidth: u64,
    #[serde(with = "chrono::serde::ts_seconds")]
    pub last_seen: chrono::DateTime<chrono::Utc>,
    pub socket_addr: SocketAddr,
}

impl Neighbor {
    pub fn is_alive(&self, dead_interval_secs: i64) -> bool {
        let now = chrono::Utc::now();
        now.signed_duration_since(self.last_seen).num_seconds() < dead_interval_secs
    }
}
