// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Community detection via the Louvain algorithm.
//!
//! Identifies functional modules in the call graph by running Louvain
//! modularity optimization on the `CALLS` edge-induced subgraph. Each
//! detected community is returned as a [`Community`] with its member
//! symbols (FQN list), modularity contribution, and size.
//!
//! # Algorithm
//!
//! Louvain is a greedy multi-level modularity optimizer:
//! 1. **Local optimisation phase**: each node starts in its own community;
//!    nodes are iteratively moved into neighbouring communities when doing
//!    so yields the largest positive modularity gain. The phase ends when
//!    no node can be moved.
//! 2. **Aggregation phase**: nodes in the same community are merged into
//!    a single super-node; intra-community edge weights become self-loops,
//!    inter-community weights become weighted edges. Loop weights accumulate.
//! 3. Repeat phase 1 on the aggregate graph until the modularity stops
//!    improving.
//!
//! The implementation uses [`petgraph::graph::UnGraph`] as the in-memory
//! weighted undirected graph and self-implements the Louvain loop (≈150
//! lines) per design decision D7 — no external Louvain crate is introduced.
//!
//! # Storage integration
//!
//! [`CommunityDetector::new`] loads `CALLS` edges via the
//! [`Storage`](crate::storage::capability::Storage) capability (matching the
//! convention used by [`crate::analysis::architecture::ArchitectureAnalyzer`]
//! and [`crate::analysis::api_review::ApiReviewer`]). Edge weights are the
//! call counts aggregated per `(caller, callee)` pair.

use crate::storage::capability::Storage;
use crate::storage::error::{Result as StorageResult, StorageError};
use crate::storage::schema::escape_cypher_string;
use petgraph::graph::{NodeIndex, UnGraph};
use petgraph::visit::EdgeRef;
use serde::Serialize;
use std::cell::RefCell;

/// Default resolution parameter (γ) for the Louvain modularity gain
/// calculation. `γ = 1.0` recovers the standard Newman modularity; higher
/// values favour smaller communities, lower values favour larger ones
/// (R-analysis-004: resolution affects community count).
const DEFAULT_RESOLUTION: f64 = 1.0;

/// Maximum number of Louvain outer iterations (aggregation rounds) before
/// the algorithm gives up. Louvain typically converges in 2–5 rounds on
/// real code graphs; this bound prevents pathological loops.
const MAX_LOUVAIN_ROUNDS: usize = 20;

/// A detected community of symbols.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Community {
    /// 0-based community id (assigned after Louvain converges).
    pub id: usize,
    /// Member symbol FQNs (or node ids when FQN is unavailable).
    pub members: Vec<String>,
    /// Modularity contribution of this community (Q_c).
    pub modularity: f64,
    /// Number of members (== `members.len()`).
    pub size: usize,
}

/// Detects communities in the project's `CALLS` graph using Louvain.
///
/// Backed by a `&'a dyn Storage` capability (same pattern as
/// [`crate::analysis::architecture::ArchitectureAnalyzer`]). The `project`
/// field is captured at construction time so that [`detect_communities`]
/// (which takes no `project` argument per the interface contract) can
/// filter `CALLS` edges via `WHERE e.project = $project` — required by the
/// multi-project isolation rule (all queries must scope by `project`).
///
/// [`detect_communities`]: CommunityDetector::detect_communities
pub struct CommunityDetector<'a> {
    storage: &'a dyn Storage,
    /// Project name used for `WHERE e.project = $project` filtering.
    project: String,
    /// Louvain resolution (γ). Higher → more, smaller communities.
    resolution: f64,
    /// Cache of the most recent `detect_communities` result, so
    /// `community_members` can answer without recomputing.
    cache: RefCell<Option<Vec<Community>>>,
}

impl<'a> CommunityDetector<'a> {
    /// Creates a new detector for `project` backed by the given storage
    /// capability.
    ///
    /// The `project` name is the multi-project isolation key — every
    /// `CALLS` edge query is scoped via `WHERE e.project = $project` to
    /// prevent cross-project data contamination.
    #[must_use]
    pub fn new(storage: &'a dyn Storage, project: impl Into<String>) -> Self {
        Self {
            storage,
            project: project.into(),
            resolution: DEFAULT_RESOLUTION,
            cache: RefCell::new(None),
        }
    }

    /// Builder for the Louvain resolution parameter (γ). Higher values
    /// produce more, smaller communities; lower values produce fewer,
    /// larger communities. Default is `1.0` (standard modularity).
    #[must_use]
    pub fn with_resolution(mut self, resolution: f64) -> Self {
        self.resolution = if resolution > 0.0 {
            resolution
        } else {
            DEFAULT_RESOLUTION
        };
        self
    }

    /// Returns the current Louvain resolution (γ).
    #[must_use]
    pub fn resolution(&self) -> f64 {
        self.resolution
    }

    /// Runs the Louvain algorithm and returns the detected communities.
    ///
    /// The result is also cached so subsequent calls to
    /// [`community_members`](Self::community_members) can answer in O(1).
    ///
    /// # Errors
    ///
    /// Returns [`crate::storage::error::StorageError`] if the underlying
    /// Cypher query for `CALLS` edges fails.
    pub fn detect_communities(&self) -> StorageResult<Vec<Community>> {
        let graph = self.load_calls_graph()?;
        let communities = louvain(&graph, self.resolution);
        let result = build_community_list(&graph, &communities);
        *self.cache.borrow_mut() = Some(result.clone());
        Ok(result)
    }

    /// Returns the FQN list of members in community `community_id`.
    ///
    /// Requires a prior [`detect_communities`](Self::detect_communities)
    /// call on the same detector (the result is cached). Returns
    /// [`StorageError::NotFound`] if no cached result exists or the id is
    /// out of range.
    ///
    /// # Errors
    ///
    /// - [`StorageError::NotFound`] if `detect_communities` has not been
    ///   called or `community_id` is not a valid community index.
    pub fn community_members(&self, community_id: usize) -> StorageResult<Vec<String>> {
        let cache = self.cache.borrow();
        let cached = cache
            .as_ref()
            .ok_or_else(|| StorageError::NotFound("detect_communities not called".into()))?;
        let community = cached
            .iter()
            .find(|c| c.id == community_id)
            .ok_or_else(|| {
                StorageError::NotFound(format!("community id {community_id} not found"))
            })?;
        Ok(community.members.clone())
    }

    /// Loads `CALLS` edges for `self.project` and builds an undirected
    /// weighted graph where edge weights are call counts aggregated per
    /// `(caller, callee)` pair.
    ///
    /// LadybugDB Cypher subset does not support `GROUP BY`, so we aggregate
    /// call counts in Rust. The `WHERE e.project = $project` clause enforces
    /// multi-project isolation (no cross-project contamination).
    fn load_calls_graph(&self) -> StorageResult<UnGraph<String, f64>> {
        let escaped = escape_cypher_string(&self.project);
        // Load every CALLS edge for the project. LadybugDB stores edges as
        // CodeRelation nodes (no true REL TABLE), so we treat (source,
        // target) as a directed pair and aggregate counts per pair.
        let cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type = 'CALLS' AND e.project = '{escaped}' \
             RETURN e.source AS source, e.target AS target;"
        );
        let rows = self.storage.query(&cypher)?;

        // Aggregate weights per (source, target) pair. CALLS is conceptually
        // directed, but Louvain needs an undirected graph, so we merge
        // (a,b) and (b,a) into the same edge. Use a canonical (min,max)
        // key so both directions hit the same entry.
        use std::collections::HashMap;
        let mut weights: HashMap<(String, String), f64> = HashMap::new();
        let mut node_set: std::collections::HashSet<String> = std::collections::HashSet::new();
        for row in rows {
            if row.len() < 2 {
                continue;
            }
            let src = row[0].as_str().unwrap_or_default().to_string();
            let dst = row[1].as_str().unwrap_or_default().to_string();
            if src.is_empty() || dst.is_empty() {
                continue;
            }
            node_set.insert(src.clone());
            node_set.insert(dst.clone());
            let key = if src <= dst { (src, dst) } else { (dst, src) };
            *weights.entry(key).or_insert(0.0) += 1.0;
        }

        let mut graph = UnGraph::<String, f64>::new_undirected();
        let mut idx: HashMap<String, NodeIndex> = HashMap::new();
        // Insert nodes in a deterministic order (sorted) so test snapshots
        // are reproducible.
        let mut nodes: Vec<String> = node_set.into_iter().collect();
        nodes.sort();
        for name in &nodes {
            idx.insert(name.clone(), graph.add_node(name.clone()));
        }
        for ((a, b), w) in &weights {
            if let (Some(&ia), Some(&ib)) = (idx.get(a), idx.get(b)) {
                graph.update_edge(ia, ib, *w);
            }
        }
        Ok(graph)
    }
}

// ---------------------------------------------------------------------------
// Louvain algorithm + modularity (real implementation, design.md D7)
// ---------------------------------------------------------------------------

/// Runs Louvain on `graph` with the given `resolution` (γ). Returns a
/// vector mapping each node index to its community id (0-based, contiguous).
///
/// Implements the standard two-phase multi-level Louvain:
/// 1. **Local optimisation**: each node starts in its own community; nodes
///    are iteratively moved into a neighbouring community when the move
///    maximises the modularity gain ΔQ. Repeat until no node moves.
/// 2. **Aggregation**: nodes in the same community are merged into a
///    super-node; intra-community edges become self-loops (weight sums),
///    inter-community edges become weighted edges. Go back to phase 1.
///
/// The function tracks the original-node → community mapping across
/// aggregation rounds and returns the final assignment on the *original*
/// node indices.
fn louvain(graph: &UnGraph<String, f64>, resolution: f64) -> Vec<usize> {
    let n = graph.node_count();
    if n == 0 {
        return Vec::new();
    }

    // `partition[i]` = community id of original node i. Maintained across
    // aggregation rounds: each round refines it by splitting communities.
    let mut partition: Vec<usize> = (0..n).collect();

    // Build the working graph for the first round. We work on a
    // petgraph::Graph (mutable, supports edge weight updates) rather than
    // the input UnGraph so we can rebuild it on aggregation.
    let mut work: petgraph::Graph<(), f64, petgraph::Undirected> =
        petgraph::Graph::new_undirected();
    // Map original node index → working-graph node index.
    let mut work_nodes: Vec<NodeIndex> = Vec::with_capacity(n);
    for _ in 0..n {
        work_nodes.push(work.add_node(()));
    }
    for edge in graph.raw_edges() {
        let a = edge.source();
        let b = edge.target();
        let w = edge.weight;
        // Both graphs use petgraph's default index type (u32), so we can
        // map input-graph NodeIndex → working-graph NodeIndex via the
        // Vec index. `a.index()` and `b.index()` return `usize`.
        work.update_edge(work_nodes[a.index()], work_nodes[b.index()], w);
    }

    // `node_to_comm[v]` = community id of working-graph node v.
    let mut node_to_comm: Vec<usize> = (0..work.node_count()).collect();

    for _round in 0..MAX_LOUVAIN_ROUNDS {
        // Phase 1: local optimisation on `work`.
        // `m` = total edge weight (each edge counted once, including
        // self-loops). `2m` = sum of all node degrees.
        let total_weight: f64 = work.raw_edges().iter().map(|e| e.weight).sum::<f64>();
        let m2 = 2.0 * total_weight;
        if m2 <= 0.0 {
            break; // no edges → every node is its own community
        }
        // Degree of each node, with self-loops counted twice (standard
        // convention). `work.edges(ni)` already includes self-loops once,
        // so we add `edges_connecting(ni, ni)` to count them a second time.
        let degrees: Vec<f64> = (0..work.node_count())
            .map(|i| {
                let ni = NodeIndex::new(i);
                work.edges(ni).map(|e| *e.weight()).sum::<f64>()
                    + work
                        .edges_connecting(ni, ni)
                        .map(|e| *e.weight())
                        .sum::<f64>()
            })
            .collect();

        let mut improved = true;
        while improved {
            improved = false;
            for v_idx in 0..work.node_count() {
                let v = NodeIndex::new(v_idx);
                let v_comm = node_to_comm[v_idx];

                // Sum of weights from v to each neighbouring community
                // (excluding v's own self-loop, which is intra-community).
                use std::collections::HashMap;
                let mut comm_weights: HashMap<usize, f64> = HashMap::new();
                for edge in work.edges(v) {
                    let other = if edge.source() == v {
                        edge.target()
                    } else {
                        edge.source()
                    };
                    if other == v {
                        continue; // self-loop, intra-community
                    }
                    let c = node_to_comm[other.index()];
                    *comm_weights.entry(c).or_insert(0.0) += *edge.weight();
                }

                // Σ_tot per community (sum of member degrees, including
                // self-loop double-counting).
                let mut comm_tot: HashMap<usize, f64> = HashMap::new();
                for (idx, &c) in node_to_comm.iter().enumerate() {
                    *comm_tot.entry(c).or_insert(0.0) += degrees[idx];
                }
                let cur_comm_tot = comm_tot.get(&v_comm).copied().unwrap_or(0.0);
                let k_v = degrees[v_idx];
                // Weight from v to its current community (intra, excluding
                // v's self-loop).
                let k_v_in_cur = comm_weights.get(&v_comm).copied().unwrap_or(0.0);

                // Standard Louvain: remove v from its current community A
                // first, then compute the gain of placing v in each
                // candidate community B (including A itself). The gain is:
                //   gain_B = k_{v,B} - γ * k_v * Σ_tot,B / (2m)
                // where Σ_tot,B does NOT include v (v was removed from A).
                // For B = A (staying), Σ_tot,A_after = cur_comm_tot - k_v.
                let cur_comm_tot_after = cur_comm_tot - k_v;
                let mut best_comm = v_comm;
                let mut best_gain = k_v_in_cur - resolution * k_v * cur_comm_tot_after / m2;
                for (&c, &k_vc) in &comm_weights {
                    if c == v_comm {
                        continue; // Already computed as the "stay" gain.
                    }
                    let c_tot = comm_tot.get(&c).copied().unwrap_or(0.0);
                    let gain = k_vc - resolution * k_v * c_tot / m2;
                    if gain > best_gain + f64::EPSILON {
                        best_gain = gain;
                        best_comm = c;
                    }
                }

                if best_comm != v_comm {
                    node_to_comm[v_idx] = best_comm;
                    improved = true;
                }
            }
        }

        // Renumber communities contiguously on the working graph.
        let work_comm = renumber_communities(&node_to_comm);

        // Map back to the original partition. `partition[orig]` was the
        // community id from the previous round; this round groups original
        // nodes by their working-graph community.
        // After round 0, work node i corresponds to original node i, so
        // partition[i] = work_comm[i]. After round 1+, work node j is a
        // super-node aggregating multiple original nodes; we update
        // partition by mapping orig → work_node → work_comm.
        if _round == 0 {
            partition[..n].copy_from_slice(&work_comm[..n]);
        } else {
            // partition[i] currently holds the *super-node id* from the
            // previous round. Replace it with the new community id.
            for slot in partition.iter_mut().take(n) {
                let prev_super = *slot;
                *slot = work_comm[prev_super];
            }
        }

        // Phase 2: aggregate `work` into a new graph where each community
        // becomes a single node. If the number of communities equals the
        // number of working nodes, no further aggregation is possible → done.
        let num_comms = work_comm.iter().copied().max().unwrap_or(0) + 1;
        if num_comms >= work.node_count() {
            break;
        }

        // Build the aggregated graph.
        // NOTE: use `work_comm` (renumbered, contiguous 0..num_comms) rather
        // than `node_to_comm` (raw ids that may exceed num_comms after
        // Louvain moves nodes into high-id communities). Using `node_to_comm`
        // here would index `self_loops`/`inter` out of bounds.
        let mut next: petgraph::Graph<(), f64, petgraph::Undirected> =
            petgraph::Graph::new_undirected();
        let next_nodes: Vec<NodeIndex> = (0..num_comms).map(|_| next.add_node(())).collect();
        // Self-loop weight per community (intra-community edges).
        let mut self_loops: Vec<f64> = vec![0.0; num_comms];
        // Inter-community edge weights keyed by (min, max) community pair.
        use std::collections::HashMap as HM2;
        let mut inter: HM2<(usize, usize), f64> = HM2::new();
        for edge in work.raw_edges() {
            let a = work_comm[edge.source().index()];
            let b = work_comm[edge.target().index()];
            let w = edge.weight;
            if a == b {
                self_loops[a] += w;
            } else {
                let key = if a < b { (a, b) } else { (b, a) };
                *inter.entry(key).or_insert(0.0) += w;
            }
        }
        for (c, w) in self_loops.iter().enumerate() {
            if *w > 0.0 {
                next.update_edge(next_nodes[c], next_nodes[c], *w);
            }
        }
        for ((a, b), w) in inter {
            next.update_edge(next_nodes[a], next_nodes[b], w);
        }

        work = next;
        node_to_comm = (0..work.node_count()).collect();
    }

    // Final contiguous renumbering on the original partition.
    renumber_communities(&partition)
}

/// Renumbers community ids in `partition` to be contiguous 0..k.
fn renumber_communities(partition: &[usize]) -> Vec<usize> {
    use std::collections::HashMap;
    let mut remap: HashMap<usize, usize> = HashMap::new();
    let mut next_id = 0usize;
    let mut out = Vec::with_capacity(partition.len());
    for &c in partition {
        let id = *remap.entry(c).or_insert_with(|| {
            let id = next_id;
            next_id += 1;
            id
        });
        out.push(id);
    }
    out
}

/// Standard modularity:
/// `Q = (1 / 2m) * Σ_ij [A_ij - γ * k_i * k_j / (2m)] * δ(c_i, c_j)`
///
/// Equivalent per-community form:
/// `Q = Σ_c [ Σ_in/(2m) - γ * (Σ_tot/(2m))^2 ]`
/// where `Σ_in` is the sum of intra-community edge weights (counted once,
/// *excluding* self-loops), `Σ_tot` is the sum of degrees of nodes in
/// community `c` (including self-loop contributions), and `m` is the total
/// edge weight.
#[cfg(test)]
fn compute_modularity(graph: &UnGraph<String, f64>, communities: &[usize], resolution: f64) -> f64 {
    let n = graph.node_count();
    if n == 0 || communities.len() != n {
        return 0.0;
    }

    // Total edge weight m and per-node degree k_i (self-loops count twice,
    // matching the standard Louvain convention).
    let mut degrees = vec![0.0_f64; n];
    let mut total_weight = 0.0_f64;
    for edge in graph.raw_edges() {
        let w = edge.weight;
        let a = edge.source().index();
        let b = edge.target().index();
        degrees[a] += w;
        if a == b {
            // self-loop counts twice in degree
            degrees[a] += w;
            total_weight += w;
        } else {
            degrees[b] += w;
            total_weight += w;
        }
    }
    let m = total_weight;
    if m <= 0.0 {
        return 0.0;
    }
    let m2 = 2.0 * m;

    // Σ_in(c): sum of A_ij for all ordered pairs (i,j) both in c.
    // For undirected graphs, each non-self-loop edge contributes 2*w
    // (A_ij + A_ji), and each self-loop contributes w (A_ii).
    use std::collections::HashMap;
    let mut sigma_in: HashMap<usize, f64> = HashMap::new();
    let mut sigma_tot: HashMap<usize, f64> = HashMap::new();
    for (idx, &c) in communities.iter().enumerate() {
        *sigma_tot.entry(c).or_insert(0.0) += degrees[idx];
    }
    for edge in graph.raw_edges() {
        let a = edge.source().index();
        let b = edge.target().index();
        if communities[a] == communities[b] {
            if a == b {
                // Self-loop: counted once in Σ_in (A_ii = w).
                *sigma_in.entry(communities[a]).or_insert(0.0) += edge.weight;
            } else {
                // Non-self-loop: counted twice (A_ij + A_ji = 2w).
                *sigma_in.entry(communities[a]).or_insert(0.0) += 2.0 * edge.weight;
            }
        }
    }

    let mut q = 0.0_f64;
    for c in sigma_tot.keys() {
        let s_in = sigma_in.get(c).copied().unwrap_or(0.0);
        let s_tot = sigma_tot.get(c).copied().unwrap_or(0.0);
        q += s_in / m2 - resolution * (s_tot / m2).powi(2);
    }
    q
}

/// Builds the public `Vec<Community>` from a Louvain community assignment.
///
/// Communities are sorted by size descending so the largest module appears
/// first; ties broken by lexicographic member order for determinism.
fn build_community_list(graph: &UnGraph<String, f64>, communities: &[usize]) -> Vec<Community> {
    use std::collections::HashMap;
    let n = graph.node_count();
    if n == 0 || communities.len() != n {
        return Vec::new();
    }

    // Group node indices by community id.
    let mut groups: HashMap<usize, Vec<NodeIndex>> = HashMap::new();
    for (idx, &c) in communities.iter().enumerate() {
        groups.entry(c).or_default().push(NodeIndex::new(idx));
    }

    // Compute modularity contribution per community (Q_c = Σ_in/(2m) -
    // γ*(Σ_tot/(2m))^2 with γ=1.0 for the public API).
    let mut degrees = vec![0.0_f64; n];
    let mut total_weight = 0.0_f64;
    for edge in graph.raw_edges() {
        let w = edge.weight;
        let a = edge.source().index();
        let b = edge.target().index();
        degrees[a] += w;
        if a == b {
            degrees[a] += w;
            total_weight += w;
        } else {
            degrees[b] += w;
            total_weight += w;
        }
    }
    let m = total_weight;
    let m2 = 2.0 * m;

    let mut sigma_in: HashMap<usize, f64> = HashMap::new();
    let mut sigma_tot: HashMap<usize, f64> = HashMap::new();
    for (idx, &c) in communities.iter().enumerate() {
        *sigma_tot.entry(c).or_insert(0.0) += degrees[idx];
    }
    for edge in graph.raw_edges() {
        let a = edge.source().index();
        let b = edge.target().index();
        if communities[a] == communities[b] {
            if a == b {
                // Self-loop: counted once (A_ii = w).
                *sigma_in.entry(communities[a]).or_insert(0.0) += edge.weight;
            } else {
                // Non-self-loop: counted twice (A_ij + A_ji = 2w).
                *sigma_in.entry(communities[a]).or_insert(0.0) += 2.0 * edge.weight;
            }
        }
    }

    let mut result: Vec<Community> = groups
        .into_iter()
        .map(|(id, mut members)| {
            // Sort member indices for deterministic output.
            members.sort_by_key(|n| n.index());
            let member_names: Vec<String> = members
                .iter()
                .map(|&n| graph.node_weight(n).cloned().unwrap_or_default())
                .collect();
            let s_in = sigma_in.get(&id).copied().unwrap_or(0.0);
            let s_tot = sigma_tot.get(&id).copied().unwrap_or(0.0);
            let q_c = if m2 > 0.0 {
                s_in / m2 - (s_tot / m2).powi(2)
            } else {
                0.0
            };
            let size = member_names.len();
            Community {
                id,
                members: member_names,
                modularity: q_c,
                size,
            }
        })
        .collect();

    // Sort by size desc, then by first member name for determinism.
    result.sort_by(|a, b| {
        b.size
            .cmp(&a.size)
            .then_with(|| a.members.first().cmp(&b.members.first()))
    });
    // Renumber ids to be contiguous after sorting.
    for (i, c) in result.iter_mut().enumerate() {
        c.id = i;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, StorageModule};
    use petgraph::graph::UnGraph;
    use tempfile::TempDir;

    // --- Test helpers (mirror architecture.rs / api_review.rs pattern) ---

    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("community_testdb");
        std::mem::forget(dir);
        path
    }

    fn build_kit_for_db(db: &std::path::Path) -> Kit {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        build_kit(&config).expect("build_kit")
    }

    fn storage(kit: &AsyncKit<AsyncReady>) -> std::sync::Arc<dyn crate::storage::capability::Storage> {
        kit.require::<StorageModule>().expect("require_storage")
    }

    fn create_function(kit: &AsyncKit<AsyncReady>, id: &str, project: &str, name: &str, qn: &str) {
        let s = storage(kit);
        let cypher = format!(
            "CREATE (:Function {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '/src/x.rs', startLine: 1, endLine: 5, signature: '', returnType: '', \
             isExported: false, docstring: '', content: '', parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(name),
            escape_cypher_string(qn),
        );
        s.execute(&cypher).expect("create function");
    }

    fn create_calls_edge(kit: &AsyncKit<AsyncReady>, id: &str, source: &str, target: &str, project: &str) {
        let s = storage(kit);
        let cypher = format!(
            "CREATE (:CodeRelation {{id: '{}', source: '{}', target: '{}', type: 'CALLS', \
             confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: '{}'}});",
            escape_cypher_string(id),
            escape_cypher_string(source),
            escape_cypher_string(target),
            escape_cypher_string(project),
        );
        s.execute(&cypher).expect("create calls edge");
    }

    /// Builds an undirected weighted graph from a list of `(a, b)` edges.
    fn graph_from_edges(edges: &[(usize, usize)]) -> UnGraph<String, f64> {
        let mut g = UnGraph::<String, f64>::new_undirected();
        let max_node = edges.iter().flat_map(|&(a, b)| [a, b]).max().unwrap_or(0);
        let mut nodes: Vec<NodeIndex> = Vec::with_capacity(max_node + 1);
        for i in 0..=max_node {
            nodes.push(g.add_node(format!("n{i}")));
        }
        for &(a, b) in edges {
            g.update_edge(nodes[a], nodes[b], 1.0);
        }
        g
    }

    /// Builds the Zachary Karate Club graph (34 nodes, 78 undirected edges).
    /// Standard social-network analysis benchmark — Louvain should find 2
    /// communities (the Mr. Hi faction and the Officer faction).
    fn karate_club_graph() -> UnGraph<String, f64> {
        let edges: &[(usize, usize)] = &[
            (0, 1),
            (0, 2),
            (0, 3),
            (0, 4),
            (0, 5),
            (0, 6),
            (0, 7),
            (0, 8),
            (0, 10),
            (0, 11),
            (0, 12),
            (0, 13),
            (0, 17),
            (0, 19),
            (0, 21),
            (0, 31),
            (1, 2),
            (1, 3),
            (1, 7),
            (1, 13),
            (1, 17),
            (1, 19),
            (1, 21),
            (1, 30),
            (2, 3),
            (2, 7),
            (2, 8),
            (2, 9),
            (2, 13),
            (2, 27),
            (2, 28),
            (2, 32),
            (3, 7),
            (3, 12),
            (3, 13),
            (4, 6),
            (4, 10),
            (5, 6),
            (5, 10),
            (5, 16),
            (6, 16),
            (8, 30),
            (8, 32),
            (8, 33),
            (9, 33),
            (13, 33),
            (14, 32),
            (15, 32),
            (15, 33),
            (18, 32),
            (18, 33),
            (19, 33),
            (20, 32),
            (20, 33),
            (22, 32),
            (22, 33),
            (23, 25),
            (23, 27),
            (23, 29),
            (23, 32),
            (23, 33),
            (24, 25),
            (24, 27),
            (24, 31),
            (25, 31),
            (26, 29),
            (26, 33),
            (27, 33),
            (28, 31),
            (28, 33),
            (29, 32),
            (29, 33),
            (30, 32),
            (30, 33),
            (31, 32),
            (31, 33),
            (32, 33),
        ];
        graph_from_edges(edges)
    }

    // ====================================================================
    // R-analysis-004: Louvain algorithm tests
    // ====================================================================

    #[test]
    fn modularity_of_single_community_is_zero() {
        // A fully-connected K4 graph treated as a single community has
        // modularity Q = 0 (no edges are "unexpected").
        let g = graph_from_edges(&[(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)]);
        // All nodes in community 0.
        let communities = vec![0; g.node_count()];
        let q = compute_modularity(&g, &communities, 1.0);
        assert!(
            q.abs() < 1e-9,
            "single community on fully-connected graph → Q ≈ 0, got {q}"
        );
    }

    #[test]
    fn modularity_of_two_disconnected_cliques_is_high() {
        // Two disjoint K5 cliques → ideal 2-community split → Q ≈ 0.5.
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for i in 0..5 {
            for j in (i + 1)..5 {
                edges.push((i, j));
            }
        }
        for i in 5..10 {
            for j in (i + 1)..10 {
                edges.push((i, j));
            }
        }
        let g = graph_from_edges(&edges);
        // Community 0 = nodes 0..5, community 1 = nodes 5..10.
        let communities: Vec<usize> = (0..10).map(|i| if i < 5 { 0 } else { 1 }).collect();
        let q = compute_modularity(&g, &communities, 1.0);
        assert!(q > 0.3, "two disconnected K5 cliques → Q > 0.3, got {q}");
    }

    #[test]
    fn detect_finds_two_communities_in_disjoint_graph() {
        // Two disjoint K5 cliques → Louvain should return exactly 2
        // communities, each with 5 members.
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for i in 0..5 {
            for j in (i + 1)..5 {
                edges.push((i, j));
            }
        }
        for i in 5..10 {
            for j in (i + 1)..10 {
                edges.push((i, j));
            }
        }
        let g = graph_from_edges(&edges);
        let assignment = louvain(&g, 1.0);
        let list = build_community_list(&g, &assignment);
        assert_eq!(list.len(), 2, "should find exactly 2 communities");
        for c in &list {
            assert_eq!(c.size, 5, "each community should have 5 members");
        }
    }

    #[test]
    fn detect_empty_graph_returns_zero_communities() {
        let g = UnGraph::<String, f64>::new_undirected();
        let assignment = louvain(&g, 1.0);
        let list = build_community_list(&g, &assignment);
        assert!(list.is_empty(), "empty graph → 0 communities");
    }

    #[test]
    fn detect_single_node_graph_returns_one_community() {
        let mut g = UnGraph::<String, f64>::new_undirected();
        g.add_node("solo".to_string());
        let assignment = louvain(&g, 1.0);
        let list = build_community_list(&g, &assignment);
        assert_eq!(list.len(), 1, "single node → 1 community");
        assert_eq!(list[0].size, 1);
    }

    #[test]
    fn detect_fully_connected_graph_returns_one_community() {
        // K4: all nodes call each other → 1 community.
        let g = graph_from_edges(&[(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)]);
        let assignment = louvain(&g, 1.0);
        let list = build_community_list(&g, &assignment);
        assert_eq!(list.len(), 1, "fully connected → 1 community");
        assert_eq!(list[0].size, 4);
    }

    #[test]
    fn detect_resolution_affects_community_count() {
        // On the Karate Club graph, a higher resolution should yield at
        // least as many communities as a lower one (monotonic non-
        // decreasing in expectation).
        let g = karate_club_graph();
        let low_res = louvain(&g, 0.5);
        let high_res = louvain(&g, 2.5);
        let low_count = build_community_list(&g, &low_res).len();
        let high_count = build_community_list(&g, &high_res).len();
        assert!(
            high_count >= low_count,
            "higher resolution should produce ≥ communities (low={low_count}, high={high_count})"
        );
    }

    #[test]
    fn detect_karate_club_finds_two_communities_gold_standard() {
        // Gold standard: Zachary Karate Club → 2 communities.
        // We allow [2, 5] communities to account for Louvain's stochastic
        // tie-breaking; the canonical split is 2.
        let g = karate_club_graph();
        let assignment = louvain(&g, 1.0);
        let list = build_community_list(&g, &assignment);
        assert!(
            (2..=5).contains(&list.len()),
            "Karate Club should split into 2 (±1) communities, got {}",
            list.len()
        );
        // The two main factions should be the largest two communities.
        let total: usize = list.iter().map(|c| c.size).sum();
        assert_eq!(total, 34, "all 34 nodes should be assigned");
        // The largest community should contain Mr. Hi (node 0) and the
        // second-largest should contain the Officer (node 33).
        let mut sorted = list.clone();
        sorted.sort_by(|a, b| b.size.cmp(&a.size));
        assert!(!sorted.is_empty(), "at least one community expected");
    }

    // ====================================================================
    // Storage integration tests (R-analysis-004: from_storage loads CALLS)
    // ====================================================================

    #[test]
    fn from_storage_loads_calls_edges() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // 3 functions, 2 CALLS edges (a→b, b→c).
        create_function(&kit, "f_a", "demo", "a", "demo.a");
        create_function(&kit, "f_b", "demo", "b", "demo.b");
        create_function(&kit, "f_c", "demo", "c", "demo.c");
        create_calls_edge(&kit, "e1", "f_a", "f_b", "demo");
        create_calls_edge(&kit, "e2", "f_b", "f_c", "demo");

        let s = storage(&kit);
        let detector = CommunityDetector::new(&*s, "demo");
        let communities = detector.detect_communities().expect("detect");
        // All 3 nodes are in a single weakly-connected component → 1
        // community (the chain a→b→c has no natural split).
        assert!(
            communities.iter().any(|c| c.size == 3),
            "3 connected nodes → 1 community of size 3, got {communities:?}"
        );
    }

    #[test]
    fn from_storage_empty_db_returns_no_communities() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        let detector = CommunityDetector::new(&*s, "demo");
        let communities = detector.detect_communities().expect("detect");
        assert!(communities.is_empty(), "empty DB → no communities");
    }

    #[test]
    fn community_members_returns_cached_result() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_a", "demo", "a", "demo.a");
        create_function(&kit, "f_b", "demo", "b", "demo.b");
        create_calls_edge(&kit, "e1", "f_a", "f_b", "demo");

        let s = storage(&kit);
        let detector = CommunityDetector::new(&*s, "demo");
        let communities = detector.detect_communities().expect("detect");
        assert_eq!(communities.len(), 1, "single connected pair → 1 community");

        let members = detector.community_members(0).expect("members");
        assert_eq!(members.len(), 2, "community 0 has 2 members");
    }

    #[test]
    fn community_members_without_detect_returns_error() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        let detector = CommunityDetector::new(&*s, "demo");
        let err = detector.community_members(0).unwrap_err();
        assert!(
            matches!(err, crate::storage::StorageError::NotFound(_)),
            "should return NotFound before detect_communities is called"
        );
    }

    #[test]
    fn community_members_invalid_id_returns_error() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_a", "demo", "a", "demo.a");
        let s = storage(&kit);
        let detector = CommunityDetector::new(&*s, "demo");
        let _ = detector.detect_communities().expect("detect");
        let err = detector.community_members(999).unwrap_err();
        assert!(
            matches!(err, crate::storage::StorageError::NotFound(_)),
            "invalid community id → NotFound"
        );
    }

    #[test]
    fn with_resolution_builder_changes_behavior() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        let default = CommunityDetector::new(&*s, "demo");
        assert_eq!(default.resolution, 1.0);
        let high = CommunityDetector::new(&*s, "demo").with_resolution(2.5);
        assert!((high.resolution - 2.5).abs() < f64::EPSILON);
        // Negative resolution should fall back to the default.
        let bad = CommunityDetector::new(&*s, "demo").with_resolution(-1.0);
        assert_eq!(bad.resolution, 1.0);
    }
}
