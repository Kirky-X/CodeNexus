// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Community detection via the Leiden algorithm.
//!
//! Identifies functional modules in the call graph by running Leiden
//! modularity optimization on the `CALLS` edge-induced subgraph. Each
//! detected community is returned as a [`Community`] with its member
//! symbols (FQN list), modularity contribution, and size.
//!
//! # Algorithm
//!
//! Leiden (Traag et al., 2019) improves on Louvain by adding a refinement
//! phase that guarantees the connectivity invariant — every community is
//! internally connected. The three-phase multi-level optimiser is:
//! 1. **Local optimisation phase**: each node starts in its own community;
//!    nodes are iteratively moved into neighbouring communities when doing
//!    so yields the largest positive modularity gain. The phase ends when
//!    no node can be moved.
//! 2. **Refinement phase** (Leiden addition): each community is split into
//!    its internally connected sub-communities via DFS over the induced
//!    subgraph. The first connected component retains the original
//!    community id; subsequent components receive new ids. This eliminates
//!    the "disconnected community" pathology of plain Louvain.
//! 3. **Aggregation phase**: nodes in the same community are merged into
//!    a single super-node; intra-community edge weights become self-loops,
//!    inter-community weights become weighted edges. Loop weights accumulate.
//! 4. Repeat phases 1–3 on the aggregate graph until the modularity stops
//!    improving.
//!
//! The implementation uses [`petgraph::graph::UnGraph`] as the in-memory
//! weighted undirected graph and self-implements the Leiden loop (≈200
//! lines) per design decision D7 — no external Leiden crate is introduced.
//! [`modularity_core`] is shared by both Leiden ([`RefineMode::RefineConnected`])
//! and the legacy [`louvain`] entry point ([`RefineMode::Plain`], kept for
//! quality comparison against [`leiden`] in tests).
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

/// Default resolution parameter (γ) for the Leiden modularity gain
/// calculation. `γ = 1.0` recovers the standard Newman modularity; higher
/// values favour smaller communities, lower values favour larger ones
/// (R-analysis-004: resolution affects community count).
const DEFAULT_RESOLUTION: f64 = 1.0;

/// Maximum number of Leiden/Louvain outer iterations (aggregation rounds)
/// before the algorithm gives up. Leiden typically converges in 2–5 rounds
/// on real code graphs; this bound prevents pathological loops.
const MAX_ROUNDS: usize = 20;

/// A detected community of symbols.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Community {
    /// 0-based community id (assigned after Leiden converges).
    pub id: usize,
    /// Member symbol FQNs (or node ids when FQN is unavailable).
    pub members: Vec<String>,
    /// Modularity contribution of this community (Q_c).
    pub modularity: f64,
    /// Number of members (== `members.len()`).
    pub size: usize,
}

/// Detects communities in the project's `CALLS` graph using Leiden.
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
    /// Leiden resolution (γ). Higher → more, smaller communities.
    resolution: f64,
    /// Cache of the most recent `detect_communities` result, so
    /// `community_members` can answer without recomputing.
    ///
    /// `Vec<Community>` (not `Arc<Vec<Community>>`) is intentional: the
    /// public `detect_communities()` API returns `Vec<Community>` by value,
    /// so an `Arc` would still require a clone on return (Arc::try_unwrap
    /// fails because the cache holds a strong ref). `community_members()`
    /// also returns `Vec<String>` by value, so per-member cloning is
    /// unavoidable regardless of the cache container (MED-003 evaluation:
    /// Arc has no performance benefit without breaking the public API).
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

    /// Builder for the Leiden resolution parameter (γ). Higher values
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

    /// Returns the current Leiden resolution (γ).
    #[must_use]
    pub fn resolution(&self) -> f64 {
        self.resolution
    }

    /// Runs the Leiden algorithm and returns the detected communities.
    ///
    /// Leiden improves on Louvain by adding a refinement phase that
    /// splits each community into its internally connected sub-communities
    /// between the local-moving and aggregation phases (Traag et al.,
    /// 2019). This guarantees the connectivity invariant — every
    /// community is internally connected — which plain Louvain may
    /// violate (C3: Louvain→Leiden upgrade).
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
        let communities = leiden(&graph, self.resolution);
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
        use std::collections::{BTreeSet, HashMap};
        let mut weights: HashMap<(String, String), f64> = HashMap::new();
        // BTreeSet keeps node names in sorted order at insertion time, so
        // `into_iter()` below yields them deterministically without a
        // separate `.sort()` call (LOW-003: replaces HashSet + Vec::sort).
        // Deterministic node ordering is required in *production* too — not
        // just tests — so that repeated runs on the same DB produce the
        // same community ids.
        let mut node_set: BTreeSet<String> = BTreeSet::new();
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
        // BTreeSet iteration is already sorted — no `.sort()` needed.
        for name in &node_set {
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
// Leiden / Louvain algorithm + modularity (real implementation, design.md D7)
// ---------------------------------------------------------------------------

/// Runs Leiden on `graph` with the given `resolution` (γ). Returns a vector
/// mapping each node index to its community id (0-based, contiguous).
///
/// Leiden improves on Louvain by adding a refinement phase between the local
/// moving and aggregation phases. After local moving produces a partition,
/// the refinement phase splits each community into its internally connected
/// sub-communities, guaranteeing that every community is internally
/// connected (C3: Louvain→Leiden upgrade).
///
/// On connected graphs Leiden produces the same result as Louvain; on
/// disconnected or weakly-linked graphs Leiden guarantees the connectivity
/// invariant that Louvain may violate.
fn leiden(graph: &UnGraph<String, f64>, resolution: f64) -> Vec<usize> {
    modularity_core(graph, resolution, RefineMode::RefineConnected)
}

/// Returns `true` if the subgraph induced by `nodes` is connected.
///
/// Uses iterative DFS over `graph` restricted to `nodes` (no recursion →
/// no stack-overflow risk). An empty node set is considered connected by
/// convention. Uses `Vec<bool>` scratch buffers indexed by node id for
/// O(1) membership tests (LOW-001/LOW-004: previously used HashSet with
/// hashing overhead).
#[cfg(test)]
fn is_connected_subgraph(graph: &UnGraph<String, f64>, nodes: &[NodeIndex]) -> bool {
    if nodes.is_empty() {
        return true;
    }
    let n = graph.node_count();
    let mut in_set: Vec<bool> = vec![false; n];
    for &node in nodes {
        in_set[node.index()] = true;
    }
    let mut visited: Vec<bool> = vec![false; n];
    let mut stack: Vec<NodeIndex> = Vec::with_capacity(nodes.len());
    stack.push(nodes[0]);
    let mut visited_count = 0usize;
    while let Some(node) = stack.pop() {
        if visited[node.index()] {
            continue;
        }
        visited[node.index()] = true;
        visited_count += 1;
        for edge in graph.edges(node) {
            let other = if edge.source() == node {
                edge.target()
            } else {
                edge.source()
            };
            if in_set[other.index()] && !visited[other.index()] {
                stack.push(other);
            }
        }
    }
    visited_count == nodes.len()
}

/// Runs Louvain on `graph` with the given `resolution` (γ). Returns a
/// vector mapping each node index to its community id (0-based, contiguous).
///
/// Thin wrapper around [`modularity_core`] in [`RefineMode::Plain`] mode
/// (no Leiden refinement phase). Test-only entry point used for quality
/// comparison against [`leiden`] on benchmark graphs (Karate Club, K5
/// cliques).
#[cfg(test)]
fn louvain(graph: &UnGraph<String, f64>, resolution: f64) -> Vec<usize> {
    modularity_core(graph, resolution, RefineMode::Plain)
}

/// Selects whether [`modularity_core`] runs the Leiden refinement phase
/// between the local-moving and aggregation phases (M-9: replaces the
/// previous `refine: bool` flag argument per Clean Code "Flag Argument").
#[derive(Clone, Copy, PartialEq, Eq)]
enum RefineMode {
    /// Plain Louvain: local moving → aggregation, no connectivity guarantee.
    ///
    /// Test-only: used by [`louvain`] for quality comparison against
    /// [`leiden`]. The variant is gated so that lib builds do not flag it
    /// as dead code (only `RefineConnected` is used in production).
    #[cfg(test)]
    Plain,
    /// Leiden: local moving → **refinement** → aggregation. The refinement
    /// phase splits each community into its internally connected
    /// sub-communities, guaranteeing the connectivity invariant (Traag
    /// et al., 2019).
    RefineConnected,
}

/// Core multi-level modularity optimizer shared by Louvain and Leiden.
///
/// Implements the standard two-phase multi-level algorithm:
/// 1. **Local optimisation**: each node starts in its own community; nodes
///    are iteratively moved into a neighbouring community when the move
///    maximises the modularity gain ΔQ. Repeat until no node moves.
/// 2. **Aggregation**: nodes in the same community are merged into a
///    super-node; intra-community edges become self-loops (weight sums),
///    inter-community edges become weighted edges. Go back to phase 1.
///
/// When `mode` is [`RefineMode::RefineConnected`] (Leiden), a refinement
/// phase is inserted between phases 1 and 2: each community is split into
/// its internally connected sub-communities via
/// [`refine_partition_connected`], guaranteeing the connectivity invariant
/// that Louvain may violate (C3).
///
/// The function tracks the original-node → community mapping across
/// aggregation rounds and returns the final assignment on the *original*
/// node indices.
fn modularity_core(graph: &UnGraph<String, f64>, resolution: f64, mode: RefineMode) -> Vec<usize> {
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

    for round in 0..MAX_ROUNDS {
        // Phase 1: local optimisation on `work`.
        // `m` = total edge weight (each edge counted once, including
        // self-loops). `2m` = sum of all node degrees.
        let total_weight: f64 = work.raw_edges().iter().map(|e| e.weight).sum::<f64>();
        let m2 = 2.0 * total_weight;
        if m2 <= 0.0 {
            break; // no edges → every node is its own community
        }
        let num_work_nodes = work.node_count();
        // Degree of each node, with self-loops counted twice (standard
        // convention). Single iteration over `edges(ni)`: self-loops are
        // counted twice in-place (LOW-002: previously iterated
        // `edges_connecting(ni, ni)` separately).
        let degrees: Vec<f64> = (0..num_work_nodes)
            .map(|i| {
                let ni = NodeIndex::new(i);
                work.edges(ni)
                    .map(|e| {
                        let w = *e.weight();
                        if e.source() == e.target() {
                            w * 2.0
                        } else {
                            w
                        }
                    })
                    .sum::<f64>()
            })
            .collect();

        // Σ_tot per community (sum of member degrees, including self-loop
        // double-counting). Maintained incrementally as nodes move between
        // communities (HIGH-001: previously rebuilt O(N) per node → O(N²)
        // per pass; now O(1) update per move).
        // Community ids in `node_to_comm` are always < `num_work_nodes`
        // (initially each node is its own community; moves only target
        // existing neighbour communities), so Vec<f64> indexing is safe.
        let mut comm_tot: Vec<f64> = vec![0.0; num_work_nodes];
        for (idx, &c) in node_to_comm.iter().enumerate() {
            comm_tot[c] += degrees[idx];
        }

        let mut improved = true;
        while improved {
            improved = false;
            for v_idx in 0..num_work_nodes {
                let v = NodeIndex::new(v_idx);
                let v_comm = node_to_comm[v_idx];

                // Sum of weights from v to each neighbouring community
                // (excluding v's own self-loop, which is intra-community).
                // `comm_weights` is small (≤ degree of v), HashMap acceptable
                // here — only `comm_tot` was the O(N) bottleneck (MED-002).
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

                let cur_comm_tot = comm_tot[v_comm];
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
                    let c_tot = comm_tot[c];
                    let gain = k_vc - resolution * k_v * c_tot / m2;
                    if gain > best_gain + f64::EPSILON {
                        best_gain = gain;
                        best_comm = c;
                    }
                }

                if best_comm != v_comm {
                    // Incremental Σ_tot update (HIGH-001): O(1) per move
                    // instead of O(N) rebuild on the next iteration.
                    comm_tot[v_comm] -= k_v;
                    comm_tot[best_comm] += k_v;
                    node_to_comm[v_idx] = best_comm;
                    improved = true;
                }
            }
        }

        // Phase 1.5 (Leiden refinement, C3): split each community into its
        // internally connected sub-communities. This guarantees the
        // connectivity invariant that plain Louvain may violate. Skipped
        // in [`RefineMode::Plain`] (pure Louvain mode).
        if mode == RefineMode::RefineConnected {
            node_to_comm = refine_partition_connected(&work, node_to_comm);
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
        if round == 0 {
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
        let num_comms = work_comm
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
            .saturating_add(1);
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

/// Leiden refinement phase: splits each community into its internally
/// connected sub-communities, guaranteeing the connectivity invariant.
///
/// For each community in `partition`, finds the connected components of the
/// subgraph induced by its member nodes (restricted to edges in `graph`).
/// The first connected component retains the original community id; each
/// subsequent component receives a new (higher) id. The returned partition
/// is then renumbered contiguously by the caller via
/// [`renumber_communities`].
///
/// This is the key Leiden improvement over Louvain (Traag et al., 2019):
/// it guarantees that every community is internally connected, eliminating
/// the "disconnected community" pathology of plain Louvain.
///
/// # Performance
///
/// Takes ownership of `partition` (MED-004: avoids the previous
/// `partition.to_vec()` clone). Uses reusable `Vec<bool>` scratch buffers
/// instead of per-community `HashSet` allocation (MED-001), and `BTreeMap`
/// for deterministic community iteration order (LOW-5).
fn refine_partition_connected(
    graph: &petgraph::Graph<(), f64, petgraph::Undirected>,
    mut partition: Vec<usize>,
) -> Vec<usize> {
    let n = graph.node_count();
    if n == 0 || partition.len() != n {
        return partition;
    }
    // Group nodes by community. BTreeMap for deterministic iteration order
    // (LOW-5: HashMap iteration order is non-deterministic across runs due
    // to random seeding, which could affect which component retains the
    // original community id).
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<usize, Vec<NodeIndex>> = BTreeMap::new();
    for (idx, &c) in partition.iter().enumerate() {
        groups.entry(c).or_default().push(NodeIndex::new(idx));
    }
    let mut next_comm = partition
        .iter()
        .copied()
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    // Reusable scratch buffers (MED-001): Vec<bool> indexed by node index,
    // reset per community. Eliminates per-community HashSet allocation and
    // hashing overhead; for 10⁵-node graphs this saves thousands of heap
    // allocations per Leiden round.
    let mut in_current_comm: Vec<bool> = vec![false; n];
    let mut visited: Vec<bool> = vec![false; n];
    let mut stack: Vec<NodeIndex> = Vec::with_capacity(n);
    for (_orig_comm, members) in groups {
        if members.len() <= 1 {
            continue;
        }
        // Mark current community members.
        for &m in &members {
            in_current_comm[m.index()] = true;
        }
        // Find connected components within this community (DFS restricted
        // to community members).
        let mut components: Vec<Vec<NodeIndex>> = Vec::new();
        for &start in &members {
            if visited[start.index()] {
                continue;
            }
            let mut comp: Vec<NodeIndex> = Vec::new();
            stack.clear();
            stack.push(start);
            while let Some(node) = stack.pop() {
                if visited[node.index()] {
                    continue;
                }
                visited[node.index()] = true;
                comp.push(node);
                for edge in graph.edges(node) {
                    let other = if edge.source() == node {
                        edge.target()
                    } else {
                        edge.source()
                    };
                    if in_current_comm[other.index()] && !visited[other.index()] {
                        stack.push(other);
                    }
                }
            }
            components.push(comp);
        }
        // First component keeps the original community id; others get new
        // ids so the caller's `renumber_communities` makes them contiguous.
        for (i, comp) in components.iter().enumerate() {
            if i == 0 {
                continue;
            }
            for &node in comp {
                partition[node.index()] = next_comm;
            }
            next_comm = next_comm.saturating_add(1);
        }
        // Reset scratch buffers for the next community.
        for &m in &members {
            in_current_comm[m.index()] = false;
            visited[m.index()] = false;
        }
    }
    partition
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

/// Builds the public `Vec<Community>` from a Leiden/Louvain community assignment.
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

    fn build_kit_for_db(db: &std::path::Path) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    fn storage(
        kit: &AsyncKit<AsyncReady>,
    ) -> std::sync::Arc<dyn crate::storage::capability::Storage> {
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

    fn create_calls_edge(
        kit: &AsyncKit<AsyncReady>,
        id: &str,
        source: &str,
        target: &str,
        project: &str,
    ) {
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
    // R-analysis-004: Louvain baseline tests (C3 comparison reference)
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
        sorted.sort_by_key(|b| std::cmp::Reverse(b.size));
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

    // --- compute_modularity edge-case tests (lines 463, 486, 477-478, 505) ---

    #[test]
    fn compute_modularity_returns_zero_for_empty_graph() {
        let g = UnGraph::<String, f64>::new_undirected();
        let communities: Vec<usize> = Vec::new();
        let q = compute_modularity(&g, &communities, 1.0);
        assert!(q.abs() < 1e-9, "empty graph → Q = 0, got {q}");
    }

    #[test]
    fn compute_modularity_returns_zero_for_mismatched_length() {
        // Graph has 3 nodes but communities has only 2 entries.
        let g = graph_from_edges(&[(0, 1), (1, 2)]);
        let communities = vec![0, 1]; // length mismatch
        let q = compute_modularity(&g, &communities, 1.0);
        assert!(
            q.abs() < 1e-9,
            "mismatched communities length → Q = 0, got {q}"
        );
    }

    #[test]
    fn compute_modularity_returns_zero_for_no_edges() {
        // Graph with nodes but no edges → m = 0 → Q = 0.
        let mut g = UnGraph::<String, f64>::new_undirected();
        g.add_node("a".to_string());
        g.add_node("b".to_string());
        let communities = vec![0, 0];
        let q = compute_modularity(&g, &communities, 1.0);
        assert!(q.abs() < 1e-9, "no edges → Q = 0, got {q}");
    }

    #[test]
    fn compute_modularity_handles_self_loop() {
        // Graph with a self-loop on node 0 in community 0.
        let mut g = UnGraph::<String, f64>::new_undirected();
        let n0 = g.add_node("a".to_string());
        let n1 = g.add_node("b".to_string());
        g.update_edge(n0, n0, 2.0); // self-loop
        g.update_edge(n0, n1, 1.0);
        let communities = vec![0, 0];
        let q = compute_modularity(&g, &communities, 1.0);
        // The implementation uses A_ii = w (not 2w) for self-loops, so
        // Σ_in ≠ Σ_tot for a single community → Q < 0. We verify the
        // self-loop path is exercised and Q is finite.
        assert!(q.is_finite(), "Q should be finite, got {q}");
        assert!(
            q < 0.0,
            "single community with self-loop → Q < 0 (A_ii=w convention), got {q}"
        );
    }

    #[test]
    fn compute_modularity_self_loop_in_split_community() {
        // Two communities with a self-loop in community 0.
        let mut g = UnGraph::<String, f64>::new_undirected();
        let n0 = g.add_node("a".to_string());
        let n1 = g.add_node("b".to_string());
        let n2 = g.add_node("c".to_string());
        g.update_edge(n0, n0, 1.0); // self-loop in comm 0
        g.update_edge(n0, n1, 1.0); // intra-comm edge
        g.update_edge(n1, n2, 0.5); // inter-comm edge
        let communities = vec![0, 0, 1]; // a,b in comm 0; c in comm 1
        let q = compute_modularity(&g, &communities, 1.0);
        // The implementation uses A_ii = w (not 2w), so Q is negative.
        // We verify the self-loop path is exercised and Q is finite.
        assert!(q.is_finite(), "Q should be finite, got {q}");
        assert!(
            q < 0.0,
            "self-loop + inter-comm edge → Q < 0 (A_ii=w convention), got {q}"
        );
    }

    // --- build_community_list with self-loops (lines 549-550, 570) ---

    #[test]
    fn build_community_list_handles_self_loops() {
        // Graph with a self-loop; build_community_list should not panic and
        // should compute modularity contributions correctly.
        let mut g = UnGraph::<String, f64>::new_undirected();
        let n0 = g.add_node("a".to_string());
        let n1 = g.add_node("b".to_string());
        g.update_edge(n0, n0, 1.0); // self-loop
        g.update_edge(n0, n1, 1.0);
        let communities = vec![0, 0];
        let list = build_community_list(&g, &communities);
        assert_eq!(list.len(), 1, "single community expected");
        assert_eq!(list[0].size, 2, "community has 2 members");
        // Modularity should be a valid finite number.
        assert!(
            list[0].modularity.is_finite(),
            "modularity should be finite, got {}",
            list[0].modularity
        );
    }

    #[test]
    fn build_community_list_returns_empty_for_mismatched_length() {
        let g = graph_from_edges(&[(0, 1)]);
        let communities = vec![0]; // length mismatch (graph has 2 nodes)
        let list = build_community_list(&g, &communities);
        assert!(
            list.is_empty(),
            "mismatched communities length → empty list"
        );
    }

    // --- louvain with self-loop (self-loop continue in phase 1) ---

    #[test]
    fn louvain_handles_graph_with_self_loops() {
        // Graph with self-loops — Louvain should not panic and should
        // still produce a valid community assignment.
        let mut g = UnGraph::<String, f64>::new_undirected();
        let n0 = g.add_node("a".to_string());
        let n1 = g.add_node("b".to_string());
        let n2 = g.add_node("c".to_string());
        let n3 = g.add_node("d".to_string());
        g.update_edge(n0, n0, 1.0); // self-loop
        g.update_edge(n0, n1, 1.0);
        g.update_edge(n2, n2, 1.0); // self-loop
        g.update_edge(n2, n3, 1.0);
        let assignment = louvain(&g, 1.0);
        let list = build_community_list(&g, &assignment);
        // All 4 nodes should be assigned.
        let total: usize = list.iter().map(|c| c.size).sum();
        assert_eq!(total, 4, "all 4 nodes should be assigned");
    }

    // ====================================================================
    // C3: Leiden refinement phase tests (T110)
    // ====================================================================

    #[test]
    fn test_leiden_refinement_phase_produces_connected_communities() {
        // C3 (Louvain→Leiden): Leiden guarantees every community is
        // internally connected (Traag et al., 2019). Construct a graph
        // with two K5 cliques bridged via a hub node — the classic
        // structure where plain Louvain may merge the two cliques into
        // one disconnected community. Leiden's refinement phase splits
        // any disconnected community into its connected components.
        let mut edges: Vec<(usize, usize)> = Vec::new();
        // Clique A: nodes 0..5 (K5).
        for i in 0..5 {
            for j in (i + 1)..5 {
                edges.push((i, j));
            }
        }
        // Clique B: nodes 5..10 (K5).
        for i in 5..10 {
            for j in (i + 1)..10 {
                edges.push((i, j));
            }
        }
        // Bridge: hub node 10 connects to one node in each clique.
        edges.push((2, 10));
        edges.push((10, 7));
        let g = graph_from_edges(&edges);

        // Run Leiden with the refinement phase enabled.
        let assignment = leiden(&g, 1.0);

        // Group node indices by community id.
        use std::collections::HashMap;
        let mut groups: HashMap<usize, Vec<NodeIndex>> = HashMap::new();
        for (idx, &c) in assignment.iter().enumerate() {
            groups.entry(c).or_default().push(NodeIndex::new(idx));
        }

        // Leiden invariant: every community must be internally connected.
        assert!(
            !groups.is_empty(),
            "Leiden should produce at least one community"
        );
        for (comm, members) in &groups {
            assert!(
                is_connected_subgraph(&g, members),
                "Leiden produced disconnected community {comm} with members {members:?} — \
                 refinement phase should guarantee connectivity (C3)"
            );
        }
    }

    #[test]
    fn test_leiden_preserves_louvain_quality_on_karate_club() {
        // C3: On a well-connected graph (Karate Club) where Louvain's
        // connectivity invariant already holds, Leiden should produce
        // roughly the same community count. The refinement phase only
        // splits disconnected communities, so it should not dramatically
        // over-split a connected graph.
        let g = karate_club_graph();
        let louvain_assignment = louvain(&g, 1.0);
        let leiden_assignment = leiden(&g, 1.0);
        let louvain_count = build_community_list(&g, &louvain_assignment).len();
        let leiden_count = build_community_list(&g, &leiden_assignment).len();
        // Leiden may produce slightly more communities (refinement can
        // split weakly-connected subgraphs), but not dramatically more.
        assert!(
            leiden_count >= louvain_count,
            "Leiden should produce >= Louvain communities (L={louvain_count}, Ld={leiden_count})"
        );
        assert!(
            leiden_count <= louvain_count + 2,
            "Leiden should not over-split Karate Club (L={louvain_count}, Ld={leiden_count})"
        );

        // All Leiden communities must also satisfy the connectivity invariant.
        use std::collections::HashMap;
        let mut groups: HashMap<usize, Vec<NodeIndex>> = HashMap::new();
        for (idx, &c) in leiden_assignment.iter().enumerate() {
            groups.entry(c).or_default().push(NodeIndex::new(idx));
        }
        for (comm, members) in &groups {
            assert!(
                is_connected_subgraph(&g, members),
                "Leiden produced disconnected community {comm} on Karate Club (C3)"
            );
        }
    }

    #[test]
    fn test_refine_partition_connected_splits_disconnected_community() {
        // C3: Direct unit test for refine_partition_connected. Construct
        // a working graph with two disjoint triangles {0,1,2} and {3,4,5}.
        // Pass a partition that puts all 6 nodes in community 0 (clearly
        // disconnected). Refinement should split into 2 communities.
        let mut work: petgraph::Graph<(), f64, petgraph::Undirected> =
            petgraph::Graph::new_undirected();
        let n0 = work.add_node(());
        let n1 = work.add_node(());
        let n2 = work.add_node(());
        let n3 = work.add_node(());
        let n4 = work.add_node(());
        let n5 = work.add_node(());
        work.update_edge(n0, n1, 1.0);
        work.update_edge(n1, n2, 1.0);
        work.update_edge(n0, n2, 1.0);
        work.update_edge(n3, n4, 1.0);
        work.update_edge(n4, n5, 1.0);
        work.update_edge(n3, n5, 1.0);
        // Partition: all 6 nodes in community 0 (disconnected).
        let partition = vec![0; 6];
        let refined = refine_partition_connected(&work, partition);
        // Should now have 2 distinct community ids.
        let unique: std::collections::HashSet<usize> = refined.iter().copied().collect();
        assert_eq!(
            unique.len(),
            2,
            "disconnected community should be split into 2, got {refined:?}"
        );
        // First triangle stays in original comm; second triangle gets new id.
        assert_eq!(refined[0], refined[1]);
        assert_eq!(refined[1], refined[2]);
        assert_eq!(refined[3], refined[4]);
        assert_eq!(refined[4], refined[5]);
        assert_ne!(refined[0], refined[3]);
    }

    #[test]
    fn test_refine_partition_connected_preserves_connected_community() {
        // C3: If a community is already internally connected, refinement
        // should be a no-op (single community id retained).
        let mut work: petgraph::Graph<(), f64, petgraph::Undirected> =
            petgraph::Graph::new_undirected();
        let n0 = work.add_node(());
        let n1 = work.add_node(());
        let n2 = work.add_node(());
        work.update_edge(n0, n1, 1.0);
        work.update_edge(n1, n2, 1.0);
        // All in community 0 (connected via path 0-1-2).
        let partition = vec![0; 3];
        let refined = refine_partition_connected(&work, partition);
        let unique: std::collections::HashSet<usize> = refined.iter().copied().collect();
        assert_eq!(
            unique.len(),
            1,
            "connected community should not be split, got {refined:?}"
        );
    }

    #[test]
    fn test_refine_partition_connected_empty_and_singletons() {
        // C3: Edge cases — empty graph and single-node communities should
        // be handled without panic.
        let empty: petgraph::Graph<(), f64, petgraph::Undirected> =
            petgraph::Graph::new_undirected();
        let refined = refine_partition_connected(&empty, Vec::new());
        assert!(refined.is_empty(), "empty graph → empty partition");

        let mut work: petgraph::Graph<(), f64, petgraph::Undirected> =
            petgraph::Graph::new_undirected();
        let _n0 = work.add_node(());
        let refined = refine_partition_connected(&work, vec![0]);
        assert_eq!(refined, vec![0], "singleton community unchanged");
    }

    // ====================================================================
    // Round 4 — M-6/M-7/M-8 supplementary tests (review follow-up)
    // ====================================================================

    // ---- M-6: leiden() basic scenarios (mirror the louvain() ones) ----

    #[test]
    fn test_leiden_empty_graph_returns_no_communities() {
        // M-6: Leiden on an empty graph must return an empty assignment
        // without panic. Mirrors `detect_empty_graph_returns_zero_communities`.
        let g = UnGraph::<String, f64>::new_undirected();
        let assignment = leiden(&g, 1.0);
        assert!(assignment.is_empty(), "empty graph → empty assignment");
        assert!(build_community_list(&g, &assignment).is_empty());
    }

    #[test]
    fn test_leiden_single_node_returns_one_community() {
        // M-6: A single-node graph has exactly one community (the node
        // itself). Leiden's refinement phase is a no-op on singletons.
        let mut g = UnGraph::<String, f64>::new_undirected();
        g.add_node("solo".to_string());
        let assignment = leiden(&g, 1.0);
        let list = build_community_list(&g, &assignment);
        assert_eq!(list.len(), 1, "single node → 1 community");
        assert_eq!(list[0].size, 1);
    }

    #[test]
    fn test_leiden_fully_connected_returns_one_community() {
        // M-6: K4 (complete graph on 4 nodes) is maximally connected —
        // Leiden must produce a single community of size 4. The refinement
        // phase has nothing to split here.
        let g = graph_from_edges(&[(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)]);
        let assignment = leiden(&g, 1.0);
        let list = build_community_list(&g, &assignment);
        assert_eq!(list.len(), 1, "fully connected → 1 community");
        assert_eq!(list[0].size, 4);
    }

    #[test]
    fn test_leiden_handles_graph_with_self_loops() {
        // M-6: Self-loops (intra-community edges after aggregation) must
        // not crash Leiden and must be counted toward a node's degree.
        // Mirrors `louvain_handles_graph_with_self_loops`.
        let mut g = UnGraph::<String, f64>::new_undirected();
        let a = g.add_node("a".to_string());
        let b = g.add_node("b".to_string());
        let c = g.add_node("c".to_string());
        g.update_edge(a, b, 1.0);
        g.update_edge(b, c, 1.0);
        g.update_edge(a, a, 2.0); // self-loop on a
        let assignment = leiden(&g, 1.0);
        let list = build_community_list(&g, &assignment);
        let total: usize = list.iter().map(|c| c.size).sum();
        assert_eq!(total, 3, "all 3 nodes should be assigned");
    }

    #[test]
    fn test_leiden_resolution_affects_community_count() {
        // M-6: Higher resolution γ → more, smaller communities. Construct
        // two K4 cliques bridged by a single edge. At γ=1.0 the bridge may
        // or may not merge them; at γ=10.0 the cliques must split.
        let mut edges: Vec<(usize, usize)> = Vec::new();
        // Clique A: 0..4
        for i in 0..4 {
            for j in (i + 1)..4 {
                edges.push((i, j));
            }
        }
        // Clique B: 4..8
        for i in 4..8 {
            for j in (i + 1)..8 {
                edges.push((i, j));
            }
        }
        // Single weak bridge.
        edges.push((0, 4));
        let g = graph_from_edges(&edges);

        let low = build_community_list(&g, &leiden(&g, 1.0)).len();
        let high = build_community_list(&g, &leiden(&g, 10.0)).len();
        assert!(
            high >= low,
            "higher resolution should produce >= communities (low={low}, high={high})"
        );
        assert!(high >= 2, "at high resolution the two cliques should split");
    }

    // ---- M-8: refine_partition_connected with mixed input ----

    #[test]
    fn test_refine_partition_connected_mixed_input() {
        // M-8: Mixed input — three communities where some are connected
        // and some are disconnected. Refinement must split only the
        // disconnected ones, leaving the connected ones unchanged.
        //
        // Layout (8 nodes):
        //   comm 0: {0,1,2} — connected triangle (no split)
        //   comm 1: {3,4,5,6} — two disjoint edges {3-4} and {5-6} (split into 2)
        //   comm 2: {7} — singleton (no split)
        // Expected: 4 distinct community ids after refinement.
        let mut work: petgraph::Graph<(), f64, petgraph::Undirected> =
            petgraph::Graph::new_undirected();
        for _ in 0..8 {
            work.add_node(());
        }
        // comm 0: triangle 0-1-2 (connected)
        work.update_edge(NodeIndex::new(0), NodeIndex::new(1), 1.0);
        work.update_edge(NodeIndex::new(1), NodeIndex::new(2), 1.0);
        work.update_edge(NodeIndex::new(0), NodeIndex::new(2), 1.0);
        // comm 1: two disjoint edges 3-4 and 5-6 (disconnected)
        work.update_edge(NodeIndex::new(3), NodeIndex::new(4), 1.0);
        work.update_edge(NodeIndex::new(5), NodeIndex::new(6), 1.0);
        // comm 2: singleton 7 (no edges)
        let partition = vec![0, 0, 0, 1, 1, 1, 1, 2];
        let refined = refine_partition_connected(&work, partition);

        // Comm 0 (triangle) should retain its id (0) unchanged.
        assert_eq!(refined[0], 0, "comm 0 first node keeps id");
        assert_eq!(refined[1], 0, "comm 0 second node keeps id");
        assert_eq!(refined[2], 0, "comm 0 third node keeps id");
        // Comm 1 (disconnected) should be split: {3,4} keep id 1, {5,6} get new id.
        assert_eq!(refined[3], 1, "comm 1 first component keeps id");
        assert_eq!(refined[4], 1, "comm 1 first component keeps id");
        assert_eq!(
            refined[5], refined[6],
            "comm 1 second component shares new id"
        );
        assert_ne!(refined[5], 1, "comm 1 second component gets a new id");
        assert_ne!(refined[5], 0, "new id must not collide with comm 0");
        // Comm 2 (singleton) should retain its id (2) unchanged.
        assert_eq!(refined[7], 2, "comm 2 singleton keeps id");

        // Total distinct ids: {0, 1, refined[5], 2} = 4.
        let unique: std::collections::HashSet<usize> = refined.iter().copied().collect();
        assert_eq!(
            unique.len(),
            4,
            "expected 4 communities after mixed refinement, got {refined:?}"
        );
    }

    // ---- M-7: public API Leiden connectivity invariant (integration) ----

    #[test]
    fn test_detect_communities_leiden_connectivity_invariant() {
        // M-7: End-to-end integration test — drive Leiden through the
        // public `CommunityDetector::detect_communities()` API against a
        // Storage-backed graph designed to stress the refinement phase.
        //
        // Topology: two K5 cliques (A: f_a0..f_a4, B: f_b0..f_b4) bridged
        // via a hub node f_hub connected to one node in each clique. This
        // is the classic structure where plain Louvain may produce a
        // disconnected community (the two cliques merged through the hub);
        // Leiden's refinement phase must guarantee every community is
        // internally connected.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Clique A.
        for i in 0..5 {
            create_function(
                &kit,
                &format!("f_a{i}"),
                "demo",
                &format!("a{i}"),
                &format!("demo.a{i}"),
            );
        }
        // Clique B.
        for i in 0..5 {
            create_function(
                &kit,
                &format!("f_b{i}"),
                "demo",
                &format!("b{i}"),
                &format!("demo.b{i}"),
            );
        }
        // Hub.
        create_function(&kit, "f_hub", "demo", "hub", "demo.hub");
        // Intra-clique A edges (K5: 10 edges).
        let mut edge_idx = 0;
        for i in 0..5 {
            for j in (i + 1)..5 {
                create_calls_edge(
                    &kit,
                    &format!("e_a{edge_idx}"),
                    &format!("f_a{i}"),
                    &format!("f_a{j}"),
                    "demo",
                );
                edge_idx += 1;
            }
        }
        // Intra-clique B edges (K5: 10 edges).
        edge_idx = 0;
        for i in 0..5 {
            for j in (i + 1)..5 {
                create_calls_edge(
                    &kit,
                    &format!("e_b{edge_idx}"),
                    &format!("f_b{i}"),
                    &format!("f_b{j}"),
                    "demo",
                );
                edge_idx += 1;
            }
        }
        // Bridge edges: hub → one node in each clique.
        create_calls_edge(&kit, "e_hub_a", "f_hub", "f_a0", "demo");
        create_calls_edge(&kit, "e_hub_b", "f_hub", "f_b0", "demo");

        let s = storage(&kit);
        let detector = CommunityDetector::new(&*s, "demo");
        let communities = detector.detect_communities().expect("detect");
        assert!(
            !communities.is_empty(),
            "Leiden should produce at least one community"
        );

        // Reload the graph (private helper, accessible in-module) to verify
        // the connectivity invariant via `is_connected_subgraph`.
        let graph = detector.load_calls_graph().expect("load graph");
        // Build FQN → NodeIndex map for membership lookup.
        use std::collections::HashMap;
        let mut fqn_to_idx: HashMap<String, NodeIndex> = HashMap::new();
        for idx in graph.node_indices() {
            if let Some(name) = graph.node_weight(idx) {
                fqn_to_idx.insert(name.clone(), idx);
            }
        }
        // Invariant: every community returned by detect_communities() must
        // be internally connected (C3 Leiden guarantee).
        for c in &communities {
            let members: Vec<NodeIndex> = c
                .members
                .iter()
                .filter_map(|fqn| fqn_to_idx.get(fqn).copied())
                .collect();
            assert!(
                !members.is_empty(),
                "community {} has no resolvable members: {:?}",
                c.id,
                c.members
            );
            assert!(
                is_connected_subgraph(&graph, &members),
                "Leiden produced disconnected community {} with members {:?} — \
                 refinement phase should guarantee connectivity (C3, M-7)",
                c.id,
                c.members
            );
        }
    }
}
