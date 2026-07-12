// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Trace and context output types.

use crate::model::Node;
use serde::{Deserialize, Serialize};

use super::facade::TraceCycle;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceResult {
    pub symbol: String,
    pub paths: Vec<TracePath>,
    pub cycles: Vec<TraceCycle>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TracePath {
    pub nodes: Vec<TraceNode>,
    pub edges: Vec<TraceEdge>,
    pub depth: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceNode {
    pub name: String,
    pub label: String,
    pub file_path: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceEdge {
    pub edge_type: String,
    pub reason: Option<String>,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextOutput {
    pub symbol: String,
    pub node: SymbolNodeOutput,
    pub incoming: Vec<RelatedNodeOutput>,
    pub outgoing: Vec<RelatedNodeOutput>,
    pub processes: Vec<RelatedNodeOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SymbolNodeOutput {
    pub name: String,
    pub label: String,
    pub qualified_name: String,
    pub file_path: Option<String>,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub language: Option<String>,
    pub signature: Option<String>,
    pub is_exported: bool,
}

impl From<&Node> for SymbolNodeOutput {
    fn from(n: &Node) -> Self {
        Self {
            name: n.name.clone(),
            label: n.label.to_string(),
            qualified_name: n.qualified_name.clone(),
            file_path: n.file_path.clone(),
            start_line: n.start_line,
            end_line: n.end_line,
            language: n.language.map(|l| l.to_string()),
            signature: n.signature.clone(),
            is_exported: n.is_exported,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RelatedNodeOutput {
    pub name: String,
    pub label: String,
    pub qualified_name: String,
    pub file_path: Option<String>,
    pub start_line: Option<u32>,
    pub edge_type: String,
    pub edge_confidence: f32,
    pub edge_reason: Option<String>,
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
            cycles: Vec::new(),
        };
        assert_eq!(result.symbol, "foo");
        assert!(result.paths.is_empty());
        assert!(result.cycles.is_empty());
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
            cycles: Vec::new(),
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

    #[test]
    fn symbol_node_output_from_node() {
        let node = Node::builder(NodeLabel::Function, "foo", "demo.foo")
            .file_path("/x.rs")
            .start_line(10)
            .end_line(15)
            .language(crate::model::Language::Rust)
            .signature("fn foo()".to_string())
            .is_exported(true)
            .build();
        let out = SymbolNodeOutput::from(&node);
        assert_eq!(out.name, "foo");
        assert_eq!(out.label, "Function");
        assert_eq!(out.qualified_name, "demo.foo");
        assert_eq!(out.file_path.as_deref(), Some("/x.rs"));
        assert_eq!(out.start_line, Some(10));
        assert_eq!(out.end_line, Some(15));
        assert_eq!(out.language.as_deref(), Some("rust"));
        assert_eq!(out.signature.as_deref(), Some("fn foo()"));
        assert!(out.is_exported);
    }

    #[test]
    fn related_nodes_sort_by_edge_type_then_name() {
        let mut v = [
            RelatedNodeOutput {
                name: "z".into(),
                label: "Function".into(),
                qualified_name: "demo.z".into(),
                file_path: None,
                start_line: None,
                edge_type: "CALLS".into(),
                edge_confidence: 0.9,
                edge_reason: None,
            },
            RelatedNodeOutput {
                name: "a".into(),
                label: "Function".into(),
                qualified_name: "demo.a".into(),
                file_path: None,
                start_line: None,
                edge_type: "CALLS".into(),
                edge_confidence: 0.5,
                edge_reason: None,
            },
            RelatedNodeOutput {
                name: "b".into(),
                label: "Module".into(),
                qualified_name: "demo.b".into(),
                file_path: None,
                start_line: None,
                edge_type: "IMPORTS".into(),
                edge_confidence: 1.0,
                edge_reason: None,
            },
        ];
        v.sort_by(|a, b| {
            a.edge_type
                .cmp(&b.edge_type)
                .then_with(|| a.name.cmp(&b.name))
        });
        assert_eq!(v[0].edge_type, "CALLS");
        assert_eq!(v[0].name, "a");
        assert_eq!(v[1].edge_type, "CALLS");
        assert_eq!(v[1].name, "z");
        assert_eq!(v[2].edge_type, "IMPORTS");
        assert_eq!(v[2].name, "b");
    }

    #[test]
    fn context_output_serializes_to_json() {
        let out = ContextOutput {
            symbol: "main".into(),
            node: SymbolNodeOutput {
                name: "main".into(),
                label: "Function".into(),
                qualified_name: "demo.main".into(),
                file_path: Some("/x.rs".into()),
                start_line: Some(1),
                end_line: Some(10),
                language: Some("rust".into()),
                signature: Some("fn main()".into()),
                is_exported: true,
            },
            incoming: vec![],
            outgoing: vec![],
            processes: vec![],
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"symbol\":\"main\""));
        assert!(json.contains("\"node\""));
        assert!(json.contains("\"incoming\""));
        assert!(json.contains("\"outgoing\""));
        assert!(json.contains("\"processes\""));
    }
}
