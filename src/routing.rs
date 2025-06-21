use petgraph::algo::astar;
use crate::network::TopoGraph;

/// Pour chaque paire (src, dst) renvoie (sysname_src, sysname_dst, chemin)
pub fn compute_best_paths(graph: &TopoGraph)
                          -> Vec<(String, String, Vec<String>)>
{
    let mut res = Vec::new();
    for si in graph.node_indices() {
        for di in graph.node_indices() {
            if si == di { continue; }
            if let Some((_, path)) =
                astar(graph, si, |g| g == di, |_| 1.0, |_| 0.0)
            {
                let hops = path.into_iter()
                    .map(|ni| graph[ni].clone())
                    .collect();
                res.push((graph[si].clone(), graph[di].clone(), hops));
            }
        }
    }
    res
}
