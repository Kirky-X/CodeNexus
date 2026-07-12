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
const MAX_NODES_LIMIT: usize = 1000;

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
                    file_path: predecessor
                        .file_path
                        .clone()
                        .unwrap_or_default(),
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
    fn assess_risk(&self, affected: &[ImpactNode]) -> RiskAssessment {
        let count = affected.len();
        let max_depth = affected.iter().map(|n| n.depth).max().unwrap_or(0);

        // Factor 1: affected count → level.
        let (count_level, count_value) = if count > 50 {
            (RiskLevel::Critical, 1.0)
        } else if count > 20 {
            (RiskLevel::High, 0.8)
        } else if count > 5 {
            (RiskLevel::Medium, 0.5)
        } else {
            (RiskLevel::Low, 0.2)
        };

        // Factor 2: depth weight (depth > 5 adds weight, depth < 3 reduces).
        let depth_weight = if max_depth > 5 {
            1.0 + (max_depth - 5) as f64 * 0.1
        } else if max_depth < 3 {
            (max_depth as f64) / 3.0 * 0.5
        } else {
            0.75
        };

        // Factor 3: edge type weight (max weight among affected nodes).
        let edge_weight = affected
            .iter()
            .map(|n| edge_type_weight(n.edge_type))
            .fold(0.0_f64, f64::max);

        let score = ((count_value * 0.5) + (depth_weight * 0.25) + (edge_weight * 0.25))
            .clamp(0.0, 1.0);

        // Final level is the higher of count-based level and score-based level.
        let level = if count > 50 || score >= 0.8 {
            RiskLevel::Critical
        } else if count > 20 || score >= 0.6 {
            RiskLevel::High
        } else if count > 5 || score >= 0.3 {
            RiskLevel::Medium
        } else {
            RiskLevel::Low
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
                description: format!("max depth {max_depth} → weight {depth_weight:.2}"),
            },
            RiskFactor {
                name: "edge_type_weight".to_string(),
                value: edge_weight,
                description: format!("max edge weight {edge_weight:.2}"),
            },
        ];

        RiskAssessment {
            level,
            score,
            factors,
        }
    }
}

/// Returns the risk weight for an edge type.
fn edge_type_weight(edge: EdgeType) -> f64 {
    match edge {
        EdgeType::Calls => 1.0,
        EdgeType::Implements => 0.8,
        EdgeType::UsesType => 0.6,
        EdgeType::HttpCalls => 0.4,
        _ => 0.3,
    }
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
}
