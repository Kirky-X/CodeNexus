// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Tracing engine (PRD §4.2, ADD §3.4).
//!
//! Performs BFS traversal over `Calls`/`FfiCalls` (call graph) and
//! `DataFlows`/`Reads`/`Writes` (data flow) edges with depth limits, plus
//! impact analysis. Exposed via the [`TraceFacade`] (Facade pattern).
//!
//! # Modules
//!
//! - [`error`]: [`TraceError`] and [`Result`](error::Result) alias.
//! - [`call_graph`]: [`CallGraphTracer`] for BFS over `Calls`/`FfiCalls`.
//! - [`data_flow`]: [`DataFlowTracer`] for BFS over `DataFlows`/`Reads`/`Writes`.
//! - [`impact`]: [`ImpactAnalyzer`] for reverse-BFS impact analysis (P1).
//! - [`facade`]: [`TraceFacade`] and [`TraceType`] (Facade pattern).
//! - [`graph_loader`]: subgraph loader (BFS expand from a symbol).
//! - [`module`]: trait-kit [`TraceModule`] / [`TraceModuleBuilder`] / [`TraceConfig`].

pub mod capability;
pub mod call_graph;
pub mod data_flow;
pub mod error;
pub mod facade;
pub mod graph_loader;
pub mod impact;
pub mod module;

pub use call_graph::CallGraphTracer;
pub use data_flow::DataFlowTracer;
pub use error::{Result, TraceError};
pub use facade::{TraceFacade, TraceType};
pub use impact::ImpactAnalyzer;
pub use module::{TraceConfig, TraceModule, TraceModuleBuilder};

use crate::model::Node;

/// The result of a trace operation (PRD §4.2.3).
#[derive(Debug, Clone, PartialEq)]
pub struct TraceResult {
    /// The queried symbol name.
    pub symbol: String,
    /// The list of trace paths discovered.
    pub paths: Vec<TracePath>,
}

/// A single trace path: a sequence of nodes and edges with a depth.
#[derive(Debug, Clone, PartialEq)]
pub struct TracePath {
    /// Nodes on the path (name + type + location).
    pub nodes: Vec<TraceNode>,
    /// Edges on the path (type + reason).
    pub edges: Vec<TraceEdge>,
    /// Path depth (number of edges).
    pub depth: usize,
}

/// A node on a trace path (PRD §4.2.3 `paths[].nodes`).
#[derive(Debug, Clone, PartialEq)]
pub struct TraceNode {
    /// Short display name of the node.
    pub name: String,
    /// The [`NodeLabel`](crate::model::NodeLabel) as a string (e.g. `"Function"`).
    pub label: String,
    /// Source file path, if known.
    pub file_path: Option<String>,
    /// 1-based start line, if known.
    pub start_line: Option<u32>,
}

impl From<&Node> for TraceNode {
    fn from(node: &Node) -> Self {
        Self {
            name: node.name.clone(),
            label: node.label.to_string(),
            file_path: node.file_path.clone(),
            start_line: node.start_line,
        }
    }
}

/// An edge on a trace path (PRD §4.2.3 `paths[].edges`).
#[derive(Debug, Clone, PartialEq)]
pub struct TraceEdge {
    /// The [`EdgeType`](crate::model::EdgeType) as a string (e.g. `"CALLS"`).
    pub edge_type: String,
    /// Human-readable reason for the edge, if any.
    pub reason: Option<String>,
    /// Confidence score in `[0.0, 1.0]`.
    pub confidence: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Node, NodeLabel};

    fn make_node() -> Node {
        Node::builder(NodeLabel::Function, "foo", "proj.foo")
            .file_path("/src/foo.rs")
            .start_line(42)
            .build()
    }

    #[test]
    fn trace_node_from_node_copies_fields() {
        let node = make_node();
        let tn = TraceNode::from(&node);
        assert_eq!(tn.name, "foo");
        assert_eq!(tn.label, "Function");
        assert_eq!(tn.file_path.as_deref(), Some("/src/foo.rs"));
        assert_eq!(tn.start_line, Some(42));
    }

    #[test]
    fn trace_node_from_node_without_location() {
        let node = Node::builder(NodeLabel::Variable, "x", "proj.x").build();
        let tn = TraceNode::from(&node);
        assert_eq!(tn.name, "x");
        assert_eq!(tn.label, "Variable");
        assert!(tn.file_path.is_none());
        assert!(tn.start_line.is_none());
    }

    #[test]
    fn trace_node_label_uses_display_form() {
        let node = Node::builder(NodeLabel::GlobalVar, "g", "proj.g").build();
        let tn = TraceNode::from(&node);
        assert_eq!(tn.label, "GlobalVar");
    }

    #[test]
    fn trace_result_symbol_and_paths() {
        let result = TraceResult {
            symbol: "foo".to_string(),
            paths: Vec::new(),
        };
        assert_eq!(result.symbol, "foo");
        assert!(result.paths.is_empty());
    }

    #[test]
    fn trace_path_depth_zero_empty() {
        let path = TracePath {
            nodes: vec![TraceNode::from(&make_node())],
            edges: Vec::new(),
            depth: 0,
        };
        assert_eq!(path.depth, 0);
        assert_eq!(path.nodes.len(), 1);
        assert!(path.edges.is_empty());
    }

    #[test]
    fn trace_path_with_edge() {
        let path = TracePath {
            nodes: vec![
                TraceNode::from(&make_node()),
                TraceNode::from(&Node::builder(NodeLabel::Function, "bar", "proj.bar").build()),
            ],
            edges: vec![TraceEdge {
                edge_type: "CALLS".to_string(),
                reason: Some("direct call".to_string()),
                confidence: 0.95,
            }],
            depth: 1,
        };
        assert_eq!(path.nodes.len(), 2);
        assert_eq!(path.edges.len(), 1);
        assert_eq!(path.edges[0].edge_type, "CALLS");
        assert_eq!(path.edges[0].reason.as_deref(), Some("direct call"));
        assert!((path.edges[0].confidence - 0.95).abs() < f32::EPSILON);
    }

    #[test]
    fn trace_path_clone_is_equal() {
        let path = TracePath {
            nodes: vec![TraceNode::from(&make_node())],
            edges: Vec::new(),
            depth: 0,
        };
        let cloned = path.clone();
        assert_eq!(path, cloned);
    }

    #[test]
    fn trace_result_clone_is_equal() {
        let result = TraceResult {
            symbol: "foo".to_string(),
            paths: vec![TracePath {
                nodes: vec![TraceNode::from(&make_node())],
                edges: Vec::new(),
                depth: 0,
            }],
        };
        let cloned = result.clone();
        assert_eq!(result, cloned);
    }

    #[test]
    fn trace_node_debug_contains_name() {
        let tn = TraceNode::from(&make_node());
        let s = format!("{tn:?}");
        assert!(s.contains("foo"));
        assert!(s.contains("Function"));
    }

    #[test]
    fn trace_edge_debug_contains_type() {
        let te = TraceEdge {
            edge_type: "FFI_CALLS".to_string(),
            reason: None,
            confidence: 0.7,
        };
        let s = format!("{te:?}");
        assert!(s.contains("FFI_CALLS"));
    }

    #[test]
    fn trace_edge_without_reason() {
        let te = TraceEdge {
            edge_type: "READS".to_string(),
            reason: None,
            confidence: 1.0,
        };
        assert!(te.reason.is_none());
        assert!((te.confidence - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn trace_node_equality() {
        let n1 = TraceNode::from(&make_node());
        let n2 = TraceNode::from(&make_node());
        assert_eq!(n1, n2);
        let n3 = TraceNode {
            name: "different".to_string(),
            ..n1.clone()
        };
        assert_ne!(n1, n3);
    }

    #[test]
    fn trace_edge_equality() {
        let e1 = TraceEdge {
            edge_type: "CALLS".to_string(),
            reason: Some("r".to_string()),
            confidence: 0.9,
        };
        let e2 = e1.clone();
        assert_eq!(e1, e2);
        let e3 = TraceEdge {
            edge_type: "READS".to_string(),
            ..e1.clone()
        };
        assert_ne!(e1, e3);
    }
}
