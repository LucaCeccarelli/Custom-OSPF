use petgraph::{Graph, Undirected};
use std::collections::HashMap;
use crate::neighbors::LsaTable;

/// Graphe dynamique : chaque nœud est un `String` (= sysname)
pub type TopoGraph = Graph<String, (), Undirected>;

/// Reconstruit le graphe à partir des LSAs reçus
pub async fn build_graph(lsa_tab: &LsaTable) -> TopoGraph {
    let lsa_map = lsa_tab.read().await;
    let mut g = Graph::<String, (), Undirected>::new_undirected();
    let mut idx: HashMap<String, _> = HashMap::new();

    // 1) nœuds
    for sys in lsa_map.keys() {
        let ni = g.add_node(sys.clone());
        idx.insert(sys.clone(), ni);
    }
    // 2) arêtes
    for (sys, neis) in lsa_map.iter() {
        if let Some(&si) = idx.get(sys) {
            for nei in neis {
                if let Some(&di) = idx.get(nei) {
                    // pour éviter duplicata, n’ajoute que si si < di
                    if si.index() < di.index() {
                        g.add_edge(si, di, ());
                    }
                }
            }
        }
    }
    g
}
