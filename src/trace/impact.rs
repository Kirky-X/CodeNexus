// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Impact analyzer (trace/impact.rs) — P1 explosion-radius analysis.
//!
//! Provides [`ImpactAnalyzer`] for computing the set of nodes affected by a
//! change to a given symbol. Performs a reverse BFS over all edge types so
//! that any node that (transitively) depends on the symbol is reported.

use std::collections::{HashSet, VecDeque};

use crate::model::{EdgeType, Graph, NodeId};
use serde::{Deserialize, Serialize};

use super::TraceNode;

// ===== Multi-dimensional impact types (T024-T027) =====

/// Configuration for multi-dimensional impact analysis.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImpactConfig {
    /// Maximum BFS depth (default 5, max 10).
    pub max_depth: u32,
    /// Edge types to traverse during reverse BFS.
    pub edge_types: Vec<EdgeType>,
    /// When `false`, skip TESTS edges.
    pub include_tests: bool,
}

/// Maximum allowed depth (spec constraint).
const MAX_DEPTH_LIMIT: u32 = 10;
/// Maximum nodes returned per trace (spec constraint).
///
/// B-bulwark-5: raised from 1000 to 5000 to match `MAX_SUBGRAPH_NODES` after
/// bulwark testing showed high-fanin symbols (270+ direct callers) were
/// truncated on the first BFS hop, hiding all transitive impact. The two
/// caps are intentionally aligned so that `load_graph` never loads more
/// nodes than `trace_upstream` can analyse.
const MAX_NODES_LIMIT: usize = 5000;

impl Default for ImpactConfig {
    fn default() -> Self {
        Self {
            max_depth: 5,
            edge_types: vec![EdgeType::Calls, EdgeType::Implements, EdgeType::UsesType],
            include_tests: false,
        }
    }
}

/// Risk level for an impact analysis result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskLevel {
    Critical,
    High,
    Medium,
    Low,
}

/// A single factor contributing to the risk score.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RiskFactor {
    pub name: String,
    pub value: f64,
    pub description: String,
}

/// Risk assessment for an impact analysis.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RiskAssessment {
    pub level: RiskLevel,
    pub score: f64,
    pub factors: Vec<RiskFactor>,
}

/// A node affected by a change, with propagation path info.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImpactNode {
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub impact_path: Vec<String>,
    pub edge_type: EdgeType,
    pub depth: u32,
}

/// Full impact analysis result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImpactResult {
    pub symbol: String,
    pub affected: Vec<ImpactNode>,
    pub risk_assessment: RiskAssessment,
}

/// Reverse-BFS impact analyzer (P1: change explosion radius).
///
/// Holds an immutable borrow of the [`Graph`] and exposes [`analyze`] which
/// returns every node that (transitively) reaches `symbol_id` via any edge
/// type, up to `depth` hops.
///
/// [`analyze`]: ImpactAnalyzer::analyze
pub struct ImpactAnalyzer<'a> {
    graph: &'a Graph,
    config: ImpactConfig,
}

// ===== Risk scoring constants (assess_risk) =====

/// Affected-count thresholds → weight factor, evaluated in order; first
/// threshold exceeded wins. Indices map to levels: 0→Critical, 1→High,
/// 2→Medium; the default (no match) → Low.
const RISK_COUNT_THRESHOLDS: &[(usize, f64)] = &[
    (50, 1.0), // >50 → Critical weight
    (20, 0.8), // >20 → High weight
    (5, 0.5),  // >5  → Medium weight
];
/// Count factor when affected count is ≤ the lowest threshold (Low level).
const RISK_COUNT_LOW_FACTOR: f64 = 0.08;
/// Score-formula weight for the count factor.
const RISK_WEIGHT_COUNT: f64 = 0.6;
/// Score-formula weight for the depth factor.
const RISK_WEIGHT_DEPTH: f64 = 0.2;
/// Score-formula weight for the edge factor.
const RISK_WEIGHT_EDGE: f64 = 0.2;

impl<'a> ImpactAnalyzer<'a> {
    /// Creates a new `ImpactAnalyzer` bound to the given graph.
    #[must_use]
    pub fn new(graph: &'a Graph) -> Self {
        Self {
            graph,
            config: ImpactConfig::default(),
        }
    }

    /// Creates a new `ImpactAnalyzer` with a custom [`ImpactConfig`].
    #[must_use]
    pub fn with_config(graph: &'a Graph, config: ImpactConfig) -> Self {
        Self { graph, config }
    }

    /// Performs a reverse BFS from `symbol_id` over all edge types, returning
    /// the distinct set of nodes that (transitively) depend on `symbol_id`
    /// within `depth` hops.
    ///
    /// The returned list does not include `symbol_id` itself. Each dependent
    /// node appears exactly once (deduplicated). Order is BFS order from the
    /// start node.
    ///
    /// Returns an empty vector if `symbol_id` is not in the graph or no node
    /// reaches it within `depth` hops.
    pub fn analyze(&self, symbol_id: &NodeId, depth: usize) -> Vec<TraceNode> {
        if self.graph.get_node(symbol_id).is_none() {
            return Vec::new();
        }
        let mut visited: HashSet<NodeId> = HashSet::new();
        visited.insert(symbol_id.clone());
        // Queue holds (node_id, current_depth).
        let mut queue: VecDeque<(NodeId, usize)> = VecDeque::new();
        queue.push_back((symbol_id.clone(), 0));

        let mut results: Vec<TraceNode> = Vec::new();

        while let Some((current_id, current_depth)) = queue.pop_front() {
            // Single-line for coverage: tarpaulin attribute continuation
            if current_depth >= depth {
                continue;
            }
            // Reverse traversal: find nodes whose outgoing edges point at
            // `current_id` (i.e. reverse_neighbors over all edge types).
            for predecessor in self.graph.reverse_neighbors(&current_id, None) {
                // Single-line for coverage: tarpaulin attribute continuation
                if visited.contains(&predecessor.id) {
                    continue;
                }
                visited.insert(predecessor.id.clone());
                results.push(TraceNode::from(predecessor));
                queue.push_back((predecessor.id.clone(), current_depth + 1));
            }
        }

        results
    }

    /// Multi-dimensional impact analysis: traces upstream dependents using
    /// configured edge types and assesses risk.
    ///
    /// Returns an [`ImpactResult`] with affected nodes (each recording the
    /// edge type and propagation path) and a [`RiskAssessment`].
    pub fn analyze_impact(&self, symbol_id: &NodeId) -> ImpactResult {
        let symbol_name = self
            .graph
            .get_node(symbol_id)
            .map(|n| n.name.clone())
            .unwrap_or_default();
        let affected = self.trace_upstream(symbol_id, &self.config);
        let risk_assessment = self.assess_risk(&affected);
        ImpactResult {
            symbol: symbol_name,
            affected,
            risk_assessment,
        }
    }

    /// Reverse BFS over configured edge types, returning [`ImpactNode`]s with
    /// propagation path and edge type info.
    ///
    /// Each affected node appears exactly once (deduplicated by id). Respects
    /// `config.max_depth` (clamped to [`MAX_DEPTH_LIMIT`]) and
    /// [`MAX_NODES_LIMIT`].
    fn trace_upstream(&self, start_id: &NodeId, config: &ImpactConfig) -> Vec<ImpactNode> {
        if self.graph.get_node(start_id).is_none() {
            return Vec::new();
        }
        let max_depth = config.max_depth.min(MAX_DEPTH_LIMIT);
        let edge_filter: HashSet<EdgeType> = config.edge_types.iter().copied().collect();
        let mut visited: HashSet<NodeId> = HashSet::new();
        visited.insert(start_id.clone());
        // Queue holds (node_id, depth, path_to_node, edge_type_to_reach_it).
        let mut queue: VecDeque<(NodeId, u32, Vec<String>, EdgeType)> = VecDeque::new();
        let start_name = self
            .graph
            .get_node(start_id)
            .map(|n| n.name.clone())
            .unwrap_or_default();
        queue.push_back((start_id.clone(), 0, vec![start_name], EdgeType::Calls));

        let mut results: Vec<ImpactNode> = Vec::new();

        while let Some((current_id, current_depth, path, _edge)) = queue.pop_front() {
            if current_depth >= max_depth {
                continue;
            }
            if results.len() >= MAX_NODES_LIMIT {
                break;
            }
            // Reverse traversal: find incoming edges whose type is in the filter.
            for edge in self.graph.edges_to(&current_id) {
                if !edge_filter.contains(&edge.edge_type) {
                    continue;
                }
                if !config.include_tests && edge.edge_type == EdgeType::Tests {
                    continue;
                }
                let predecessor = match self.graph.get_node(&edge.source) {
                    Some(n) => n,
                    None => continue,
                };
                if visited.contains(&predecessor.id) {
                    continue;
                }
                visited.insert(predecessor.id.clone());
                let mut new_path = path.clone();
                new_path.push(predecessor.name.clone());
                results.push(ImpactNode {
                    name: predecessor.name.clone(),
                    qualified_name: predecessor.qualified_name.clone(),
                    file_path: predecessor.file_path.clone().unwrap_or_default(),
                    impact_path: new_path.clone(),
                    edge_type: edge.edge_type,
                    depth: current_depth + 1,
                });
                queue.push_back((
                    predecessor.id.clone(),
                    current_depth + 1,
                    new_path,
                    edge.edge_type,
                ));
            }
        }

        results
    }

    /// Assesses risk based on affected node count, max depth, and edge type
    /// weights.
    ///
    /// Scoring formula:
    /// - `count_factor`: >50→1.0, 21-50→0.8, 6-20→0.5, ≤5→0.08
    /// - `depth_factor`: depth>5→0.5+(d-5)*0.1, depth 3-5→0.5, depth<3→d/3*0.3
    /// - `edge_factor`: max edge type weight among affected nodes
    /// - `score = count_factor*0.6 + depth_factor*0.2 + edge_factor*0.2`
    ///
    /// Final level = max(count-based level, score-based level).
    fn assess_risk(&self, affected: &[ImpactNode]) -> RiskAssessment {
        let count = affected.len();
        let max_depth = affected.iter().map(|n| n.depth).max().unwrap_or(0);

        // Factor 1: affected count → count_factor and count_level (table lookup).
        // Threshold index maps to level: 0→Critical, 1→High, 2→Medium; default→Low.
        let (count_factor, count_level) = RISK_COUNT_THRESHOLDS
            .iter()
            .position(|&(threshold, _)| count > threshold)
            .map(|idx| {
                let level = match idx {
                    0 => RiskLevel::Critical,
                    1 => RiskLevel::High,
                    2 => RiskLevel::Medium,
                    _ => unreachable!("RISK_COUNT_THRESHOLDS has exactly 3 entries"),
                };
                (RISK_COUNT_THRESHOLDS[idx].1, level)
            })
            .unwrap_or((RISK_COUNT_LOW_FACTOR, RiskLevel::Low));

        // Factor 2: depth factor (depth > 5 adds weight, depth < 3 reduces).
        let depth_factor = if max_depth > 5 {
            (0.5 + (max_depth - 5) as f64 * 0.1).min(1.0)
        } else if max_depth >= 3 {
            0.5
        } else {
            (max_depth as f64) / 3.0 * 0.3
        };

        // Factor 3: edge type weight (max weight among affected nodes).
        let edge_factor = affected
            .iter()
            .map(|n| edge_type_weight(n.edge_type))
            .fold(0.0_f64, f64::max);

        let score = (count_factor * RISK_WEIGHT_COUNT
            + depth_factor * RISK_WEIGHT_DEPTH
            + edge_factor * RISK_WEIGHT_EDGE)
            .clamp(0.0, 1.0);

        // Score-based level.
        let score_level = if score >= 0.8 {
            RiskLevel::Critical
        } else if score >= 0.6 {
            RiskLevel::High
        } else if score >= 0.3 {
            RiskLevel::Medium
        } else {
            RiskLevel::Low
        };

        // Final level is the higher of count-based and score-based.
        let level = if risk_level_rank(count_level) >= risk_level_rank(score_level) {
            count_level
        } else {
            score_level
        };

        let factors = vec![
            RiskFactor {
                name: "affected_count".to_string(),
                value: count as f64,
                description: format!("{count} affected nodes → {count_level:?}"),
            },
            RiskFactor {
                name: "max_depth".to_string(),
                value: max_depth as f64,
                description: format!("max depth {max_depth} → factor {depth_factor:.2}"),
            },
            RiskFactor {
                name: "edge_type_weight".to_string(),
                value: edge_factor,
                description: format!("max edge weight {edge_factor:.2}"),
            },
        ];

        RiskAssessment {
            level,
            score,
            factors,
        }
    }
}

/// Returns a numeric rank for a RiskLevel (Low=0, Medium=1, High=2, Critical=3).
fn risk_level_rank(level: RiskLevel) -> u8 {
    match level {
        RiskLevel::Low => 0,
        RiskLevel::Medium => 1,
        RiskLevel::High => 2,
        RiskLevel::Critical => 3,
    }
}

/// Default weight table for edge types (lookup by EdgeType).
const DEFAULT_EDGE_TYPE_WEIGHTS: &[(EdgeType, f64)] = &[
    (EdgeType::Calls, 1.0),
    (EdgeType::Implements, 0.8),
    (EdgeType::UsesType, 0.6),
    (EdgeType::HttpCalls, 0.4),
];
/// Fallback weight for edge types not listed in [`DEFAULT_EDGE_TYPE_WEIGHTS`].
const DEFAULT_EDGE_WEIGHT: f64 = 0.3;

/// Returns the risk weight for an edge type via table lookup.
fn edge_type_weight(edge: EdgeType) -> f64 {
    DEFAULT_EDGE_TYPE_WEIGHTS
        .iter()
        .find(|(e, _)| *e == edge)
        .map(|(_, w)| *w)
        .unwrap_or(DEFAULT_EDGE_WEIGHT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Edge, EdgeType, Node, NodeLabel};

    fn make_func(id: &str, name: &str) -> Node {
        Node::builder(NodeLabel::Function, name, format!("proj.{name}"))
            .id(id)
            .project("proj")
            .file_path(format!("src/{name}.rs"))
            .start_line(10)
            .build()
    }

    fn make_var(id: &str, name: &str) -> Node {
        Node::builder(NodeLabel::Variable, name, format!("proj.{name}"))
            .id(id)
            .project("proj")
            .build()
    }

    fn make_trait(id: &str, name: &str) -> Node {
        Node::builder(NodeLabel::Trait, name, format!("proj.{name}"))
            .id(id)
            .project("proj")
            .file_path(format!("src/{name}.rs"))
            .build()
    }

    fn make_struct(id: &str, name: &str) -> Node {
        Node::builder(NodeLabel::Struct, name, format!("proj.{name}"))
            .id(id)
            .project("proj")
            .file_path(format!("src/{name}.rs"))
            .build()
    }

    #[test]
    fn analyze_returns_callers() {
        // Reverse traversal: who calls A -> returns callers.
        // B -> A, C -> A : callers of A are B and C.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "a", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"a".to_string(), 3);
        let names: Vec<&str> = impacted.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"b"));
        assert!(names.contains(&"c"));
        assert_eq!(impacted.len(), 2);
    }

    #[test]
    fn analyze_excludes_symbol_itself() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"a".to_string(), 3);
        assert!(!impacted.iter().any(|n| n.name == "a"));
    }

    #[test]
    fn analyze_depth_limit() {
        // A <- B <- C (C calls B, B calls A). Depth 1 from A returns only B.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "b", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"a".to_string(), 1);
        assert_eq!(impacted.len(), 1);
        assert_eq!(impacted[0].name, "b");
    }

    #[test]
    fn analyze_depth_2_returns_transitive_callers() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "b", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"a".to_string(), 2);
        let names: Vec<&str> = impacted.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"b"));
        assert!(names.contains(&"c"));
        assert_eq!(impacted.len(), 2);
    }

    #[test]
    fn analyze_no_callers_returns_empty() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"b".to_string(), 3);
        assert!(impacted.is_empty());
    }

    #[test]
    fn analyze_missing_symbol_returns_empty() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"missing".to_string(), 3);
        assert!(impacted.is_empty());
    }

    #[test]
    fn analyze_zero_depth_returns_empty() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"a".to_string(), 0);
        assert!(impacted.is_empty());
    }

    #[test]
    fn analyze_deduplicates_nodes() {
        // Diamond: B -> A, C -> A, D -> B, D -> C. From A depth 3, D should
        // appear only once even though it reaches A via two paths.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_node(make_func("d", "d"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "a", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("d", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("d", "c", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"a".to_string(), 3);
        let d_count = impacted.iter().filter(|n| n.name == "d").count();
        assert_eq!(d_count, 1, "D should appear only once");
        assert_eq!(impacted.len(), 3);
    }

    #[test]
    fn analyze_follows_all_edge_types() {
        // Impact analysis follows ALL edge types, not just Calls.
        // foo reads v, bar writes v -> both foo and bar depend on v.
        let mut g = Graph::new();
        g.add_node(make_var("v", "v"));
        g.add_node(make_func("foo", "foo"));
        g.add_node(make_func("bar", "bar"));
        g.add_edge(Edge::new("foo", "v", EdgeType::Reads, "proj"));
        g.add_edge(Edge::new("bar", "v", EdgeType::Writes, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"v".to_string(), 3);
        let names: Vec<&str> = impacted.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"bar"));
    }

    #[test]
    fn analyze_cyclic_graph_terminates() {
        // A <-> B (mutual calls). Should terminate and deduplicate.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"a".to_string(), 5);
        // B calls A, so B is impacted. A calls B but A is the symbol itself.
        assert!(impacted.iter().any(|n| n.name == "b"));
        assert!(!impacted.iter().any(|n| n.name == "a"));
    }

    #[test]
    fn analyze_returns_trace_nodes_with_location() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"a".to_string(), 3);
        assert_eq!(impacted.len(), 1);
        assert_eq!(impacted[0].name, "b");
        assert_eq!(impacted[0].label, "Function");
        assert_eq!(impacted[0].file_path.as_deref(), Some("src/b.rs"));
        assert_eq!(impacted[0].start_line, Some(10));
    }

    #[test]
    fn analyze_transitive_dataflow_impact() {
        // x dataflows to y, y dataflows to z. Changing z impacts y and x.
        let mut g = Graph::new();
        g.add_node(make_var("x", "x"));
        g.add_node(make_var("y", "y"));
        g.add_node(make_var("z", "z"));
        g.add_edge(Edge::new("x", "y", EdgeType::DataFlows, "proj"));
        g.add_edge(Edge::new("y", "z", EdgeType::DataFlows, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"z".to_string(), 3);
        let names: Vec<&str> = impacted.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"y"));
        assert!(names.contains(&"x"));
        assert_eq!(impacted.len(), 2);
    }

    // ===== T024: Serialization tests for multi-dimensional impact types =====

    #[test]
    fn impact_config_default_has_expected_values() {
        let config = ImpactConfig::default();
        assert_eq!(config.max_depth, 5);
        assert_eq!(
            config.edge_types,
            vec![EdgeType::Calls, EdgeType::Implements, EdgeType::UsesType]
        );
        assert!(!config.include_tests);
    }

    #[test]
    fn impact_config_roundtrip() {
        let config = ImpactConfig {
            max_depth: 7,
            edge_types: vec![EdgeType::Calls, EdgeType::HttpCalls],
            include_tests: true,
        };
        let json = serde_json::to_string(&config).expect("serialize");
        let back: ImpactConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(config, back);
        assert!(json.contains("max_depth"));
        assert!(json.contains("include_tests"));
    }

    #[test]
    fn risk_level_roundtrip() {
        for level in [
            RiskLevel::Critical,
            RiskLevel::High,
            RiskLevel::Medium,
            RiskLevel::Low,
        ] {
            let json = serde_json::to_string(&level).expect("serialize");
            let back: RiskLevel = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(level, back);
        }
    }

    #[test]
    fn risk_factor_roundtrip() {
        let factor = RiskFactor {
            name: "affected_count".to_string(),
            value: 42.0,
            description: "42 affected nodes".to_string(),
        };
        let json = serde_json::to_string(&factor).expect("serialize");
        let back: RiskFactor = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(factor, back);
    }

    #[test]
    fn risk_assessment_roundtrip() {
        let ra = RiskAssessment {
            level: RiskLevel::High,
            score: 0.75,
            factors: vec![RiskFactor {
                name: "affected_count".to_string(),
                value: 30.0,
                description: "30 nodes".to_string(),
            }],
        };
        let json = serde_json::to_string(&ra).expect("serialize");
        let back: RiskAssessment = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ra, back);
        assert!(json.contains("level"));
        assert!(json.contains("score"));
        assert!(json.contains("factors"));
    }

    #[test]
    fn impact_node_roundtrip() {
        let node = ImpactNode {
            name: "caller".to_string(),
            qualified_name: "proj.caller".to_string(),
            file_path: "src/caller.rs".to_string(),
            impact_path: vec!["target".to_string(), "caller".to_string()],
            edge_type: EdgeType::Calls,
            depth: 1,
        };
        let json = serde_json::to_string(&node).expect("serialize");
        let back: ImpactNode = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(node, back);
        assert!(json.contains("impact_path"));
        assert!(json.contains("edge_type"));
    }

    #[test]
    fn impact_result_roundtrip() {
        let result = ImpactResult {
            symbol: "target".to_string(),
            affected: vec![ImpactNode {
                name: "caller".to_string(),
                qualified_name: "proj.caller".to_string(),
                file_path: "src/caller.rs".to_string(),
                impact_path: vec!["target".to_string(), "caller".to_string()],
                edge_type: EdgeType::Calls,
                depth: 1,
            }],
            risk_assessment: RiskAssessment {
                level: RiskLevel::Low,
                score: 0.2,
                factors: vec![],
            },
        };
        let json = serde_json::to_string(&result).expect("serialize");
        let back: ImpactResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(result, back);
        assert!(json.contains("symbol"));
        assert!(json.contains("affected"));
        assert!(json.contains("risk_assessment"));
    }

    #[test]
    fn with_config_sets_custom_config() {
        let g = Graph::new();
        let config = ImpactConfig {
            max_depth: 3,
            edge_types: vec![EdgeType::Calls],
            include_tests: true,
        };
        let analyzer = ImpactAnalyzer::with_config(&g, config);
        assert_eq!(analyzer.config.max_depth, 3);
        assert!(analyzer.config.include_tests);
    }

    #[test]
    fn new_uses_default_config() {
        let g = Graph::new();
        let analyzer = ImpactAnalyzer::new(&g);
        assert_eq!(analyzer.config.max_depth, 5);
        assert!(!analyzer.config.include_tests);
    }

    // ===== T025: Multi-edge-type upstream tracing tests =====

    #[test]
    fn trace_upstream_calls_edge_type() {
        // A calls B → changing B affects A, edge_type=Calls.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"b".to_string());
        assert_eq!(result.symbol, "b");
        assert_eq!(result.affected.len(), 1);
        assert_eq!(result.affected[0].name, "a");
        assert_eq!(result.affected[0].edge_type, EdgeType::Calls);
        assert_eq!(result.affected[0].depth, 1);
    }

    #[test]
    fn trace_upstream_implements_edge_type() {
        // A implements Trait T → changing T affects A, edge_type=Implements.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_trait("t", "t"));
        g.add_edge(Edge::new("a", "t", EdgeType::Implements, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"t".to_string());
        assert_eq!(result.affected.len(), 1);
        assert_eq!(result.affected[0].name, "a");
        assert_eq!(result.affected[0].edge_type, EdgeType::Implements);
    }

    #[test]
    fn trace_upstream_ffi_calls_edge_type() {
        // A ffi-calls B → changing B affects A, edge_type=FfiCalls.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("a", "b", EdgeType::FfiCalls, "proj"));
        let config = ImpactConfig {
            max_depth: 5,
            edge_types: vec![EdgeType::FfiCalls],
            include_tests: false,
        };
        let analyzer = ImpactAnalyzer::with_config(&g, config);
        let result = analyzer.analyze_impact(&"b".to_string());
        assert_eq!(result.affected.len(), 1);
        assert_eq!(result.affected[0].name, "a");
        assert_eq!(result.affected[0].edge_type, EdgeType::FfiCalls);
    }

    #[test]
    fn trace_upstream_http_calls_edge_type() {
        // A http-calls R → changing R affects A, edge_type=HttpCalls.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("r", "r"));
        g.add_edge(Edge::new("a", "r", EdgeType::HttpCalls, "proj"));
        let config = ImpactConfig {
            max_depth: 5,
            edge_types: vec![EdgeType::HttpCalls],
            include_tests: false,
        };
        let analyzer = ImpactAnalyzer::with_config(&g, config);
        let result = analyzer.analyze_impact(&"r".to_string());
        assert_eq!(result.affected.len(), 1);
        assert_eq!(result.affected[0].name, "a");
        assert_eq!(result.affected[0].edge_type, EdgeType::HttpCalls);
    }

    #[test]
    fn trace_upstream_filters_unconfigured_edge_types() {
        // Default config has Calls+Implements+UsesType.
        // A reads B (Reads edge) → should NOT appear in results.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_var("b", "b"));
        g.add_edge(Edge::new("a", "b", EdgeType::Reads, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"b".to_string());
        assert!(result.affected.is_empty());
    }

    #[test]
    fn trace_upstream_multi_edge_type_mixed() {
        // A calls B, C implements B, D uses-type B → all affected with correct edge types.
        let mut g = Graph::new();
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("c", "c"));
        g.add_node(make_func("d", "d"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "b", EdgeType::Implements, "proj"));
        g.add_edge(Edge::new("d", "b", EdgeType::UsesType, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"b".to_string());
        assert_eq!(result.affected.len(), 3);
        let by_name: std::collections::HashMap<&str, &ImpactNode> = result
            .affected
            .iter()
            .map(|n| (n.name.as_str(), n))
            .collect();
        assert_eq!(by_name["a"].edge_type, EdgeType::Calls);
        assert_eq!(by_name["c"].edge_type, EdgeType::Implements);
        assert_eq!(by_name["d"].edge_type, EdgeType::UsesType);
    }

    #[test]
    fn trace_upstream_records_impact_path() {
        // A → B → C (A calls B, B calls C). Changing C affects B (depth 1) and A (depth 2).
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "c", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"c".to_string());
        assert_eq!(result.affected.len(), 2);
        let b_node = result.affected.iter().find(|n| n.name == "b").unwrap();
        assert_eq!(b_node.depth, 1);
        assert_eq!(b_node.impact_path, vec!["c", "b"]);
        let a_node = result.affected.iter().find(|n| n.name == "a").unwrap();
        assert_eq!(a_node.depth, 2);
        assert_eq!(a_node.impact_path, vec!["c", "b", "a"]);
    }

    #[test]
    fn trace_upstream_no_upstream_returns_empty() {
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"a".to_string());
        assert!(result.affected.is_empty());
    }

    #[test]
    fn trace_upstream_missing_symbol_returns_empty() {
        let g = Graph::new();
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"missing".to_string());
        assert!(result.affected.is_empty());
        assert_eq!(result.symbol, "");
    }

    #[test]
    fn trace_upstream_respects_max_depth() {
        // A → B → C → D (chain of Calls). max_depth=2 from D returns C and B only.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_node(make_func("d", "d"));
        g.add_edge(Edge::new("a", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "c", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "d", EdgeType::Calls, "proj"));
        let config = ImpactConfig {
            max_depth: 2,
            edge_types: vec![EdgeType::Calls],
            include_tests: false,
        };
        let analyzer = ImpactAnalyzer::with_config(&g, config);
        let result = analyzer.analyze_impact(&"d".to_string());
        let names: Vec<&str> = result.affected.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"c"));
        assert!(names.contains(&"b"));
        assert!(!names.contains(&"a"));
        assert_eq!(result.affected.len(), 2);
    }

    // ===== T026: Type dependency impact tracing tests =====

    #[test]
    fn trace_type_dependency_finds_users() {
        // Type X is used by Function A and Function B → changing X affects both.
        let mut g = Graph::new();
        g.add_node(make_struct("x", "x"));
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("a", "x", EdgeType::UsesType, "proj"));
        g.add_edge(Edge::new("b", "x", EdgeType::UsesType, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"x".to_string());
        assert_eq!(result.symbol, "x");
        assert_eq!(result.affected.len(), 2);
        let names: Vec<&str> = result.affected.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
        for node in &result.affected {
            assert_eq!(node.edge_type, EdgeType::UsesType);
        }
    }

    #[test]
    fn trace_type_dependency_no_users_returns_empty() {
        // Type with no USES_TYPE edges → affected is empty.
        let mut g = Graph::new();
        g.add_node(make_struct("x", "x"));
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"x".to_string());
        assert!(result.affected.is_empty());
    }

    #[test]
    fn trace_type_dependency_with_qualified_name_and_file_path() {
        // Function A uses Type X → ImpactNode has correct qualified_name and file_path.
        let mut g = Graph::new();
        g.add_node(make_struct("x", "x"));
        g.add_node(make_func("a", "a"));
        g.add_edge(Edge::new("a", "x", EdgeType::UsesType, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"x".to_string());
        assert_eq!(result.affected.len(), 1);
        let node = &result.affected[0];
        assert_eq!(node.name, "a");
        assert_eq!(node.qualified_name, "proj.a");
        assert_eq!(node.file_path, "src/a.rs");
        assert_eq!(node.edge_type, EdgeType::UsesType);
        assert_eq!(node.depth, 1);
    }

    #[test]
    fn trace_type_dependency_transitive() {
        // Function A uses Type X, Function B calls A → changing X affects A (depth 1) and B (depth 2).
        let mut g = Graph::new();
        g.add_node(make_struct("x", "x"));
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("a", "x", EdgeType::UsesType, "proj"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"x".to_string());
        assert_eq!(result.affected.len(), 2);
        let a_node = result.affected.iter().find(|n| n.name == "a").unwrap();
        assert_eq!(a_node.edge_type, EdgeType::UsesType);
        assert_eq!(a_node.depth, 1);
        let b_node = result.affected.iter().find(|n| n.name == "b").unwrap();
        assert_eq!(b_node.edge_type, EdgeType::Calls);
        assert_eq!(b_node.depth, 2);
    }

    // ===== T027: Risk assessment tests =====

    #[test]
    fn risk_assessment_60_affected_is_critical() {
        // 60 functions all call target → 60 affected → Critical.
        let mut g = Graph::new();
        g.add_node(make_func("target", "target"));
        for i in 0..60 {
            let id = format!("f{i}");
            g.add_node(make_func(&id, &id));
            g.add_edge(Edge::new(&id, "target", EdgeType::Calls, "proj"));
        }
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"target".to_string());
        assert_eq!(result.affected.len(), 60);
        assert_eq!(result.risk_assessment.level, RiskLevel::Critical);
        assert!(result.risk_assessment.score >= 0.8);
    }

    #[test]
    fn risk_assessment_10_affected_depth_5_is_high() {
        // Chain of 5 (depths 1-5) + 5 direct callers (depth 1) = 10 affected, max depth 5.
        let mut g = Graph::new();
        g.add_node(make_func("target", "target"));
        // Chain: target ← c1 ← c2 ← c3 ← c4 ← c5 (depths 1-5)
        for i in 1..=5 {
            let id = format!("c{i}");
            g.add_node(make_func(&id, &id));
        }
        g.add_edge(Edge::new("c1", "target", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c2", "c1", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c3", "c2", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c4", "c3", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c5", "c4", EdgeType::Calls, "proj"));
        // 5 direct callers (depth 1)
        for i in 1..=5 {
            let id = format!("d{i}");
            g.add_node(make_func(&id, &id));
            g.add_edge(Edge::new(&id, "target", EdgeType::Calls, "proj"));
        }
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"target".to_string());
        assert_eq!(result.affected.len(), 10);
        let max_depth = result.affected.iter().map(|n| n.depth).max().unwrap_or(0);
        assert_eq!(max_depth, 5);
        assert_eq!(result.risk_assessment.level, RiskLevel::High);
    }

    #[test]
    fn risk_assessment_3_affected_depth_2_is_low() {
        // Chain of 2 (depths 1-2) + 1 direct caller (depth 1) = 3 affected, max depth 2.
        let mut g = Graph::new();
        g.add_node(make_func("target", "target"));
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        // Chain: target ← a ← b (depths 1-2)
        g.add_edge(Edge::new("a", "target", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        // Direct caller: target ← c (depth 1)
        g.add_edge(Edge::new("c", "target", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"target".to_string());
        assert_eq!(result.affected.len(), 3);
        let max_depth = result.affected.iter().map(|n| n.depth).max().unwrap_or(0);
        assert_eq!(max_depth, 2);
        assert_eq!(result.risk_assessment.level, RiskLevel::Low);
    }

    #[test]
    fn risk_assessment_score_in_valid_range() {
        // Empty affected → score should be in [0.0, 1.0].
        let mut g = Graph::new();
        g.add_node(make_func("target", "target"));
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"target".to_string());
        assert!(result.affected.is_empty());
        assert!(result.risk_assessment.score >= 0.0);
        assert!(result.risk_assessment.score <= 1.0);
    }

    #[test]
    fn risk_assessment_score_in_range_with_many_affected() {
        // 60 affected → score should still be in [0.0, 1.0].
        let mut g = Graph::new();
        g.add_node(make_func("target", "target"));
        for i in 0..60 {
            let id = format!("f{i}");
            g.add_node(make_func(&id, &id));
            g.add_edge(Edge::new(&id, "target", EdgeType::Calls, "proj"));
        }
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"target".to_string());
        assert!(result.risk_assessment.score >= 0.0);
        assert!(result.risk_assessment.score <= 1.0);
    }

    #[test]
    fn risk_assessment_factors_contain_all_three() {
        // RiskFactor list must contain affected_count, max_depth, edge_type_weight.
        let mut g = Graph::new();
        g.add_node(make_func("target", "target"));
        g.add_node(make_func("a", "a"));
        g.add_edge(Edge::new("a", "target", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"target".to_string());
        let factor_names: Vec<&str> = result
            .risk_assessment
            .factors
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert!(factor_names.contains(&"affected_count"));
        assert!(factor_names.contains(&"max_depth"));
        assert!(factor_names.contains(&"edge_type_weight"));
        // Verify factor values.
        let count_factor = result
            .risk_assessment
            .factors
            .iter()
            .find(|f| f.name == "affected_count")
            .unwrap();
        assert_eq!(count_factor.value, 1.0);
        let depth_factor = result
            .risk_assessment
            .factors
            .iter()
            .find(|f| f.name == "max_depth")
            .unwrap();
        assert_eq!(depth_factor.value, 1.0); // max depth is 1
        let edge_factor = result
            .risk_assessment
            .factors
            .iter()
            .find(|f| f.name == "edge_type_weight")
            .unwrap();
        assert_eq!(edge_factor.value, 1.0); // Calls = 1.0
    }

    #[test]
    fn risk_assessment_empty_affected_is_low() {
        let mut g = Graph::new();
        g.add_node(make_func("target", "target"));
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"target".to_string());
        assert_eq!(result.risk_assessment.level, RiskLevel::Low);
    }

    // ===== Constant guard tests (prevent accidental modification) =====

    #[test]
    fn edge_type_weight_constants_match_expected_values() {
        // Guard: DEFAULT_EDGE_TYPE_WEIGHTS must keep canonical weights.
        assert_eq!(DEFAULT_EDGE_TYPE_WEIGHTS.len(), 4);
        assert_eq!(DEFAULT_EDGE_TYPE_WEIGHTS[0], (EdgeType::Calls, 1.0));
        assert_eq!(DEFAULT_EDGE_TYPE_WEIGHTS[1], (EdgeType::Implements, 0.8));
        assert_eq!(DEFAULT_EDGE_TYPE_WEIGHTS[2], (EdgeType::UsesType, 0.6));
        assert_eq!(DEFAULT_EDGE_TYPE_WEIGHTS[3], (EdgeType::HttpCalls, 0.4));
        assert_eq!(DEFAULT_EDGE_WEIGHT, 0.3);
    }

    #[test]
    fn edge_type_weight_returns_table_value_for_listed_edges() {
        assert_eq!(edge_type_weight(EdgeType::Calls), 1.0);
        assert_eq!(edge_type_weight(EdgeType::Implements), 0.8);
        assert_eq!(edge_type_weight(EdgeType::UsesType), 0.6);
        assert_eq!(edge_type_weight(EdgeType::HttpCalls), 0.4);
    }

    #[test]
    fn edge_type_weight_returns_default_for_unlisted_edges() {
        // Reads/Writes/DataFlows/Tests/FfiCalls are not in the table → fallback.
        assert_eq!(edge_type_weight(EdgeType::Reads), DEFAULT_EDGE_WEIGHT);
        assert_eq!(edge_type_weight(EdgeType::Writes), DEFAULT_EDGE_WEIGHT);
        assert_eq!(edge_type_weight(EdgeType::DataFlows), DEFAULT_EDGE_WEIGHT);
        assert_eq!(edge_type_weight(EdgeType::Tests), DEFAULT_EDGE_WEIGHT);
        assert_eq!(edge_type_weight(EdgeType::FfiCalls), DEFAULT_EDGE_WEIGHT);
    }

    #[test]
    fn risk_scoring_constants_match_expected_values() {
        // Guard: risk thresholds and formula weights must not drift.
        assert_eq!(RISK_COUNT_THRESHOLDS.len(), 3);
        assert_eq!(RISK_COUNT_THRESHOLDS[0], (50, 1.0));
        assert_eq!(RISK_COUNT_THRESHOLDS[1], (20, 0.8));
        assert_eq!(RISK_COUNT_THRESHOLDS[2], (5, 0.5));
        assert_eq!(RISK_COUNT_LOW_FACTOR, 0.08);
        assert_eq!(RISK_WEIGHT_COUNT, 0.6);
        assert_eq!(RISK_WEIGHT_DEPTH, 0.2);
        assert_eq!(RISK_WEIGHT_EDGE, 0.2);
        // Formula weights must sum to 1.0 (score is a convex combination).
        let total = RISK_WEIGHT_COUNT + RISK_WEIGHT_DEPTH + RISK_WEIGHT_EDGE;
        assert!((total - 1.0).abs() < f64::EPSILON);
    }

    // --- Coverage gap tests: Tests edge filtering, missing predecessor, risk scoring branches ---

    #[test]
    fn trace_upstream_skips_tests_edge_when_include_tests_false() {
        // A tests B (Tests edge). Default config has include_tests=false and
        // edge_types=[Calls, Implements, UsesType]. Tests is not in the filter,
        // but even if it were, include_tests=false would skip it (line 237).
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("a", "b", EdgeType::Tests, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"b".to_string());
        assert!(result.affected.is_empty(), "Tests edge should be skipped");
    }

    #[test]
    fn trace_upstream_skips_tests_edge_when_include_tests_true_but_filter_excludes() {
        // Even with include_tests=true, if Tests is not in edge_types filter,
        // the edge is skipped via the edge_filter check (line 234).
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("a", "b", EdgeType::Tests, "proj"));
        let config = ImpactConfig {
            max_depth: 5,
            edge_types: vec![EdgeType::Calls],
            include_tests: true,
        };
        let analyzer = ImpactAnalyzer::with_config(&g, config);
        let result = analyzer.analyze_impact(&"b".to_string());
        assert!(
            result.affected.is_empty(),
            "Tests edge not in filter should be skipped"
        );
    }

    #[test]
    fn trace_upstream_includes_tests_edge_when_configured() {
        // With include_tests=true AND Tests in edge_types, the Tests edge IS
        // followed. This covers the negative branch of the include_tests check
        // (line 236 condition is false, so continue at 237 is NOT hit).
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("a", "b", EdgeType::Tests, "proj"));
        let config = ImpactConfig {
            max_depth: 5,
            edge_types: vec![EdgeType::Tests],
            include_tests: true,
        };
        let analyzer = ImpactAnalyzer::with_config(&g, config);
        let result = analyzer.analyze_impact(&"b".to_string());
        assert_eq!(result.affected.len(), 1);
        assert_eq!(result.affected[0].name, "a");
        assert_eq!(result.affected[0].edge_type, EdgeType::Tests);
    }

    #[test]
    fn trace_upstream_skips_edge_to_missing_predecessor() {
        // Edge from "missing_src" to "target", but "missing_src" node is not
        // in the graph. The predecessor lookup returns None → continue (line 241).
        let mut g = Graph::new();
        g.add_node(make_func("target", "target"));
        g.add_edge(Edge::new("missing_src", "target", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"target".to_string());
        assert!(
            result.affected.is_empty(),
            "missing predecessor should be skipped"
        );
    }

    #[test]
    fn risk_assessment_six_affected_returns_medium_count_level() {
        // 6 affected nodes → count > 5 → idx=2 → RiskLevel::Medium (line 296).
        // With depth 1 and all Calls edges:
        //   count_factor=0.5, depth_factor=1/3*0.3≈0.1, edge_factor=1.0
        //   score = 0.3 + 0.02 + 0.2 = 0.52 → score_level=Medium (line 328).
        let mut g = Graph::new();
        g.add_node(make_func("target", "target"));
        for i in 0..6 {
            let id = format!("f{i}");
            g.add_node(make_func(&id, &id));
            g.add_edge(Edge::new(&id, "target", EdgeType::Calls, "proj"));
        }
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"target".to_string());
        assert_eq!(result.affected.len(), 6);
        assert_eq!(result.risk_assessment.level, RiskLevel::Medium);
    }

    #[test]
    fn risk_assessment_deep_chain_exceeds_depth_five() {
        // Chain of 6 Calls: target ← c1 ← c2 ← c3 ← c4 ← c5 ← c6.
        // max_depth in config = 8 (clamped to 8 ≤ 10). All 6 predecessors
        // are found at depths 1-6. max_depth in results = 6 > 5, exercising
        // the depth_factor branch (0.5 + (6-5)*0.1 = 0.6) at line 304.
        let mut g = Graph::new();
        g.add_node(make_func("target", "target"));
        for i in 1..=6 {
            let id = format!("c{i}");
            g.add_node(make_func(&id, &id));
        }
        g.add_edge(Edge::new("c1", "target", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c2", "c1", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c3", "c2", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c4", "c3", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c5", "c4", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c6", "c5", EdgeType::Calls, "proj"));
        let config = ImpactConfig {
            max_depth: 8,
            edge_types: vec![EdgeType::Calls],
            include_tests: false,
        };
        let analyzer = ImpactAnalyzer::with_config(&g, config);
        let result = analyzer.analyze_impact(&"target".to_string());
        assert_eq!(result.affected.len(), 6);
        let max_depth = result.affected.iter().map(|n| n.depth).max().unwrap_or(0);
        assert_eq!(max_depth, 6);
        // depth_factor = 0.5 + (6-5)*0.1 = 0.6
        // count_factor = 0.08 (≤5 is false, 6>5 → 0.5). Wait, 6 > 5 → Medium → 0.5
        // score = 0.5*0.6 + 0.6*0.2 + 1.0*0.2 = 0.3+0.12+0.2 = 0.62 → High
        assert_eq!(result.risk_assessment.level, RiskLevel::High);
    }

    // --- Coverage gap tests: MAX_NODES_LIMIT break, visited skip, count_level=High, depth clamping ---

    #[test]
    fn trace_upstream_breaks_at_max_nodes_limit() {
        // MAX_NODES_LIMIT (5000) direct predecessors of target + 1 transitive
        // caller of f0. After adding 5000 predecessors, results.len() >=
        // MAX_NODES_LIMIT, so the break at line 229 fires before the
        // transitive caller can be found.
        let mut g = Graph::new();
        g.add_node(make_func("target", "target"));
        for i in 0..MAX_NODES_LIMIT {
            let id = format!("f{i}");
            g.add_node(make_func(&id, &id));
            g.add_edge(Edge::new(&id, "target", EdgeType::Calls, "proj"));
        }
        g.add_node(make_func("transitive", "transitive"));
        g.add_edge(Edge::new("transitive", "f0", EdgeType::Calls, "proj"));
        let config = ImpactConfig {
            max_depth: 5,
            edge_types: vec![EdgeType::Calls],
            include_tests: false,
        };
        let analyzer = ImpactAnalyzer::with_config(&g, config);
        let result = analyzer.analyze_impact(&"target".to_string());
        assert_eq!(
            result.affected.len(),
            MAX_NODES_LIMIT,
            "break should fire after MAX_NODES_LIMIT results, preventing the transitive caller"
        );
        assert!(
            !result.affected.iter().any(|n| n.name == "transitive"),
            "transitive caller should not be found due to MAX_NODES_LIMIT break"
        );
    }

    #[test]
    fn trace_upstream_skips_visited_predecessor_in_diamond() {
        // Diamond via analyze_impact (trace_upstream): D → B → A, D → C → A.
        // When processing C after B, D is already visited → skip (line 244).
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        g.add_node(make_func("d", "d"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "a", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("d", "b", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("d", "c", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"a".to_string());
        let d_count = result.affected.iter().filter(|n| n.name == "d").count();
        assert_eq!(d_count, 1, "D should appear only once (visited skip)");
        assert_eq!(result.affected.len(), 3);
    }

    #[test]
    fn risk_assessment_25_affected_returns_high_count_level() {
        // 25 affected nodes → count > 20 → idx=1 → RiskLevel::High (line 294).
        // count_factor=0.8, depth_factor=1/3*0.3≈0.1, edge_factor=1.0
        // score = 0.8*0.6 + 0.1*0.2 + 1.0*0.2 = 0.48+0.02+0.2 = 0.7 → High
        // Both count_level and score_level are High.
        let mut g = Graph::new();
        g.add_node(make_func("target", "target"));
        for i in 0..25 {
            let id = format!("f{i}");
            g.add_node(make_func(&id, &id));
            g.add_edge(Edge::new(&id, "target", EdgeType::Calls, "proj"));
        }
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"target".to_string());
        assert_eq!(result.affected.len(), 25);
        assert_eq!(result.risk_assessment.level, RiskLevel::High);
    }

    #[test]
    fn with_config_clamps_max_depth_to_limit() {
        // max_depth=15 should be clamped to MAX_DEPTH_LIMIT (10) internally.
        // Chain of 11: target ← c1 ← ... ← c11 (depths 1-11).
        // With clamped max_depth=10, only c1..c10 (depths 1-10) are found;
        // c11 at depth 11 is NOT reached because depth 10 >= 10 triggers continue.
        let mut g = Graph::new();
        g.add_node(make_func("target", "target"));
        for i in 1..=11 {
            let id = format!("c{i}");
            g.add_node(make_func(&id, &id));
        }
        for i in 1..=10 {
            let from = format!("c{i}");
            let to = if i == 1 {
                "target".to_string()
            } else {
                format!("c{}", i - 1)
            };
            g.add_edge(Edge::new(&from, &to, EdgeType::Calls, "proj"));
        }
        g.add_edge(Edge::new("c11", "c10", EdgeType::Calls, "proj"));
        let config = ImpactConfig {
            max_depth: 15,
            edge_types: vec![EdgeType::Calls],
            include_tests: false,
        };
        let analyzer = ImpactAnalyzer::with_config(&g, config);
        let result = analyzer.analyze_impact(&"target".to_string());
        assert_eq!(
            result.affected.len(),
            10,
            "max_depth=15 should be clamped to 10, finding only 10 nodes"
        );
        let max_depth = result.affected.iter().map(|n| n.depth).max().unwrap_or(0);
        assert_eq!(max_depth, 10);
        assert!(
            !result.affected.iter().any(|n| n.name == "c11"),
            "c11 at depth 11 should not be found due to clamping"
        );
    }

    #[test]
    fn trace_upstream_skips_tests_edge_when_in_filter_but_include_tests_false() {
        // Cover `!config.include_tests && edge.edge_type == EdgeType::Tests`
        // true branch (line 236-238) when Tests IS in the edge_types filter.
        // The existing test `trace_upstream_skips_tests_edge_when_include_tests_false`
        // uses the default config where Tests is NOT in the filter, so the
        // edge_filter check (line 233) skips it first. This test explicitly
        // adds Tests to the filter so the include_tests check is reached.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("a", "b", EdgeType::Tests, "proj"));
        let config = ImpactConfig {
            max_depth: 5,
            edge_types: vec![EdgeType::Tests],
            include_tests: false,
        };
        let analyzer = ImpactAnalyzer::with_config(&g, config);
        let result = analyzer.analyze_impact(&"b".to_string());
        assert!(
            result.affected.is_empty(),
            "Tests edge in filter but include_tests=false should still be skipped"
        );
    }

    #[test]
    fn risk_assessment_low_count_but_medium_score() {
        // 5 affected (≤5 → Low count_level), depth 5 (≥3 → depth_factor=0.5),
        // Calls edges (edge_factor=1.0).
        // score = 0.08*0.6 + 0.5*0.2 + 1.0*0.2 = 0.348 → Medium.
        // Final level = max(Low, Medium) = Medium (score_level wins).
        let mut g = Graph::new();
        g.add_node(make_func("target", "target"));
        for i in 1..=5 {
            let id = format!("c{i}");
            g.add_node(make_func(&id, &id));
        }
        g.add_edge(Edge::new("c1", "target", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c2", "c1", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c3", "c2", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c4", "c3", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c5", "c4", EdgeType::Calls, "proj"));
        let config = ImpactConfig {
            max_depth: 8,
            edge_types: vec![EdgeType::Calls],
            include_tests: false,
        };
        let analyzer = ImpactAnalyzer::with_config(&g, config);
        let result = analyzer.analyze_impact(&"target".to_string());
        assert_eq!(result.affected.len(), 5);
        let max_depth = result.affected.iter().map(|n| n.depth).max().unwrap_or(0);
        assert_eq!(max_depth, 5);
        assert_eq!(result.risk_assessment.level, RiskLevel::Medium);
    }

    // --- Additional coverage: max_depth=0, predecessor without file_path, ---
    // --- count_level vs score_level divergence, depth clamp boundary      ---

    #[test]
    fn trace_upstream_max_depth_zero_returns_empty() {
        // max_depth=0: the initial queue item has current_depth=0 and
        // 0 >= 0 → continue immediately. No predecessors are found even
        // though upstream edges exist.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        let config = ImpactConfig {
            max_depth: 0,
            edge_types: vec![EdgeType::Calls],
            include_tests: false,
        };
        let analyzer = ImpactAnalyzer::with_config(&g, config);
        let result = analyzer.analyze_impact(&"a".to_string());
        assert!(
            result.affected.is_empty(),
            "max_depth=0 should find nothing"
        );
        assert_eq!(result.risk_assessment.level, RiskLevel::Low);
    }

    #[test]
    fn analyze_impact_predecessor_without_file_path_uses_default() {
        // Predecessor is a Variable (make_var sets no file_path). The
        // ImpactNode's file_path should be "" via unwrap_or_default().
        let mut g = Graph::new();
        g.add_node(make_func("target", "target"));
        g.add_node(make_var("v", "v"));
        g.add_edge(Edge::new("v", "target", EdgeType::UsesType, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"target".to_string());
        assert_eq!(result.affected.len(), 1);
        assert_eq!(result.affected[0].name, "v");
        assert_eq!(
            result.affected[0].file_path, "",
            "Variable with no file_path → default empty string"
        );
    }

    #[test]
    fn risk_assessment_count_critical_score_high_count_wins() {
        // 51 affected with HttpCalls edges → count > 50 → count_level=Critical.
        // edge_factor=0.4 (HttpCalls), depth_factor=0.1 (depth 1 < 3).
        // score = 1.0*0.6 + 0.1*0.2 + 0.4*0.2 = 0.708 → score_level=High.
        // Final level = max(Critical, High) = Critical (count_level wins).
        let mut g = Graph::new();
        g.add_node(make_func("target", "target"));
        for i in 0..51 {
            let id = format!("f{i}");
            g.add_node(make_func(&id, &id));
            g.add_edge(Edge::new(&id, "target", EdgeType::HttpCalls, "proj"));
        }
        let config = ImpactConfig {
            max_depth: 5,
            edge_types: vec![EdgeType::HttpCalls],
            include_tests: false,
        };
        let analyzer = ImpactAnalyzer::with_config(&g, config);
        let result = analyzer.analyze_impact(&"target".to_string());
        assert_eq!(result.affected.len(), 51);
        assert_eq!(result.risk_assessment.level, RiskLevel::Critical);
        // Verify score is in High range (< 0.8) to confirm divergence.
        assert!(result.risk_assessment.score < 0.8);
        assert!(result.risk_assessment.score >= 0.6);
    }

    #[test]
    fn risk_assessment_depth_factor_at_clamp_boundary() {
        // Chain of 10 (max allowed by MAX_DEPTH_LIMIT). max_depth=10 →
        // depth_factor = 0.5 + (10-5)*0.1 = 1.0 (exactly at .min(1.0) clamp).
        let mut g = Graph::new();
        g.add_node(make_func("target", "target"));
        for i in 1..=10 {
            let id = format!("c{i}");
            g.add_node(make_func(&id, &id));
        }
        for i in 1..=10 {
            let from = format!("c{i}");
            let to = if i == 1 {
                "target".to_string()
            } else {
                format!("c{}", i - 1)
            };
            g.add_edge(Edge::new(&from, &to, EdgeType::Calls, "proj"));
        }
        let config = ImpactConfig {
            max_depth: 10,
            edge_types: vec![EdgeType::Calls],
            include_tests: false,
        };
        let analyzer = ImpactAnalyzer::with_config(&g, config);
        let result = analyzer.analyze_impact(&"target".to_string());
        assert_eq!(result.affected.len(), 10);
        let max_depth = result.affected.iter().map(|n| n.depth).max().unwrap_or(0);
        assert_eq!(max_depth, 10);
        // depth_factor=1.0, count=10>5→Medium(0.5), edge_factor=1.0
        // score = 0.5*0.6 + 1.0*0.2 + 1.0*0.2 = 0.7 → High
        assert_eq!(result.risk_assessment.level, RiskLevel::High);
    }

    #[test]
    fn analyze_self_loop_skips_already_visited_start() {
        // A calls A (self-loop). reverse_neighbors returns A, but A is already
        // in visited → skipped. Result is empty.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_edge(Edge::new("a", "a", EdgeType::Calls, "proj"));
        let analyzer = ImpactAnalyzer::new(&g);
        let impacted = analyzer.analyze(&"a".to_string(), 3);
        assert!(
            impacted.is_empty(),
            "self-loop should be skipped (start already visited)"
        );
    }

    // --- assess_risk: mixed edge types (fold max) ---

    #[test]
    fn assess_risk_mixed_edge_types_picks_max_weight() {
        // Two affected nodes: one with Calls (1.0), one with HttpCalls (0.4).
        // edge_factor should be max(1.0, 0.4) = 1.0.
        let mut g = Graph::new();
        g.add_node(make_func("target", "target"));
        g.add_node(make_func("caller1", "caller1"));
        g.add_node(make_func("caller2", "caller2"));
        g.add_edge(Edge::new("caller1", "target", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("caller2", "target", EdgeType::HttpCalls, "proj"));
        let config = ImpactConfig {
            max_depth: 5,
            edge_types: vec![EdgeType::Calls, EdgeType::HttpCalls],
            include_tests: false,
        };
        let analyzer = ImpactAnalyzer::with_config(&g, config);
        let result = analyzer.analyze_impact(&"target".to_string());
        assert_eq!(result.affected.len(), 2);
        let edge_factor = result
            .risk_assessment
            .factors
            .iter()
            .find(|f| f.name == "edge_type_weight")
            .expect("should have edge_type_weight factor");
        assert!(
            (edge_factor.value - 1.0).abs() < 1e-6,
            "edge_factor should be max(1.0, 0.4) = 1.0, got {}",
            edge_factor.value
        );
    }

    #[test]
    fn edge_type_weight_returns_default_for_extends_and_has_property() {
        // Extends and HasProperty are not in the weight table → fallback 0.3.
        assert_eq!(edge_type_weight(EdgeType::Extends), DEFAULT_EDGE_WEIGHT);
        assert_eq!(edge_type_weight(EdgeType::HasProperty), DEFAULT_EDGE_WEIGHT);
        assert_eq!(
            edge_type_weight(EdgeType::HandlesRoute),
            DEFAULT_EDGE_WEIGHT
        );
    }

    #[test]
    fn analyze_impact_with_symbol_name_preserves_name() {
        // When the symbol IS in the graph, analyze_impact should return its
        // name (not empty string). This covers the normal (non-default) path
        // of `get_node(symbol_id).map(|n| n.name.clone()).unwrap_or_default()`.
        let mut g = Graph::new();
        g.add_node(make_func("my_func", "my_func"));
        let analyzer = ImpactAnalyzer::new(&g);
        let result = analyzer.analyze_impact(&"my_func".to_string());
        assert_eq!(
            result.symbol, "my_func",
            "symbol name should be preserved when node exists"
        );
    }

    // ====================================================================
    // Coverage gap: empty edge_types filter, max_depth=1 boundary,
    // DataFlows fallback edge weight through analyze_impact
    // ====================================================================

    #[test]
    fn trace_upstream_empty_edge_types_returns_empty() {
        // With an empty edge_types filter, no edges match (edge_filter is
        // empty), so trace_upstream returns empty even though upstream Calls
        // edges exist. This exercises the edge_filter.contains() false branch
        // for every incoming edge.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        let config = ImpactConfig {
            max_depth: 5,
            edge_types: vec![],
            include_tests: false,
        };
        let analyzer = ImpactAnalyzer::with_config(&g, config);
        let result = analyzer.analyze_impact(&"a".to_string());
        assert!(
            result.affected.is_empty(),
            "empty edge_types should match no edges"
        );
    }

    #[test]
    fn trace_upstream_max_depth_one_finds_only_direct_predecessors() {
        // max_depth=1 boundary: only direct predecessors (depth 1) are found.
        // Transitive predecessors at depth 2 are not, because when
        // current_depth=1 >= max_depth=1, the continue fires.
        let mut g = Graph::new();
        g.add_node(make_func("a", "a"));
        g.add_node(make_func("b", "b"));
        g.add_node(make_func("c", "c"));
        // C → B → A (chain)
        g.add_edge(Edge::new("b", "a", EdgeType::Calls, "proj"));
        g.add_edge(Edge::new("c", "b", EdgeType::Calls, "proj"));
        let config = ImpactConfig {
            max_depth: 1,
            edge_types: vec![EdgeType::Calls],
            include_tests: false,
        };
        let analyzer = ImpactAnalyzer::with_config(&g, config);
        let result = analyzer.analyze_impact(&"a".to_string());
        assert_eq!(result.affected.len(), 1);
        assert_eq!(result.affected[0].name, "b");
        assert_eq!(result.affected[0].depth, 1);
        assert!(
            !result.affected.iter().any(|n| n.name == "c"),
            "C at depth 2 should not be found with max_depth=1"
        );
    }

    #[test]
    fn analyze_impact_with_data_flows_uses_fallback_edge_weight() {
        // DataFlows is NOT in DEFAULT_EDGE_TYPE_WEIGHTS, so edge_type_weight
        // returns the fallback DEFAULT_EDGE_WEIGHT (0.3). With a config that
        // includes DataFlows in edge_types, the edge is followed and the
        // edge_factor in assess_risk should be 0.3 (the fallback).
        let mut g = Graph::new();
        g.add_node(make_var("target", "target"));
        g.add_node(make_var("src", "src"));
        g.add_edge(Edge::new("src", "target", EdgeType::DataFlows, "proj"));
        let config = ImpactConfig {
            max_depth: 5,
            edge_types: vec![EdgeType::DataFlows],
            include_tests: false,
        };
        let analyzer = ImpactAnalyzer::with_config(&g, config);
        let result = analyzer.analyze_impact(&"target".to_string());
        assert_eq!(result.affected.len(), 1);
        assert_eq!(result.affected[0].name, "src");
        assert_eq!(result.affected[0].edge_type, EdgeType::DataFlows);
        let edge_factor = result
            .risk_assessment
            .factors
            .iter()
            .find(|f| f.name == "edge_type_weight")
            .expect("should have edge_type_weight factor");
        assert!(
            (edge_factor.value - DEFAULT_EDGE_WEIGHT).abs() < 1e-6,
            "DataFlows should use fallback weight {}, got {}",
            DEFAULT_EDGE_WEIGHT,
            edge_factor.value
        );
    }
}
