use std::collections::{HashMap, HashSet};
use crate::neighbors::NeighborManager;
use crate::routing::compute_routes;

#[derive(Clone)]
pub struct NetworkGraph {
    sysname: String,
    graph: HashMap<String, Vec<String>>,
}

impl NetworkGraph {
    pub fn new(sysname: String) -> Self {
        Self {
            sysname,
            graph: HashMap::new(),
        }
    }

    pub fn update(&mut self, node: String, neighbors: Vec<String>) {
        self.graph.insert(node, neighbors);
    }

    pub fn recalculate_routes(&self, neighbor_mgr: &NeighborManager, local: &str) {
        let mut graph = self.graph.clone();
        graph.insert(local.to_string(), neighbor_mgr.current());
        let routes = compute_routes(&graph, local);
        println!("=== Changement de topologie détecté, nouveau calcul de routes ===");
        for (dest, path) in routes {
            println!("Route vers {} via {:?}", dest, path);
        }
    }
}
