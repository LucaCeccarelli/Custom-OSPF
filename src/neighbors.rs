use std::{collections::HashMap, sync::{Arc, Mutex}, time::{Duration, Instant}};

#[derive(Clone)]
pub struct NeighborManager {
    neighbors: Arc<Mutex<HashMap<String, Instant>>>,
    sysname: String,
}

impl NeighborManager {
    pub fn new(sysname: String) -> Self {
        Self {
            neighbors: Arc::new(Mutex::new(HashMap::new())),
            sysname,
        }
    }

    pub fn update(&self, neighbor: String) {
        let mut map = self.neighbors.lock().unwrap();
        println!("VOISIN AJOUTÉ/MIS À JOUR: {}", neighbor);
        map.insert(neighbor, Instant::now());
    }

    pub fn purge_inactive(&self) {
        let mut map = self.neighbors.lock().unwrap();
        let now = Instant::now();
        map.retain(|n, &mut t| {
            let alive = now.duration_since(t) < Duration::from_secs(15);
            if !alive {
                println!("VOISIN SUPPRIMÉ: {}", n);
            }
            alive
        });
    }

    pub fn current(&self) -> Vec<String> {
        let map = self.neighbors.lock().unwrap();
        map.keys().cloned().collect()
    }
}
