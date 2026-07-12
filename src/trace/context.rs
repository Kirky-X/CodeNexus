// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Context graph traversal helpers for the `context` command.

use super::types::RelatedNodeOutput;
use crate::model::{EdgeType, Graph, NodeId};

pub fn resolve_start_id(graph: &Graph, symbol: &str) -> Option<NodeId> {
    let by_name: Vec<&crate::model::Node> =
        graph.nodes.values().filter(|n| n.name == symbol).collect();
    if by_name.len() == 1 {
        return Some(by_name[0].id.clone());
    }
    let by_qn: Vec<&crate::model::Node> = graph
        .nodes
        .values()
        .filter(|n| n.qualified_name == symbol)
        .collect();
    if by_qn.len() == 1 {
        return Some(by_qn[0].id.clone());
    }
    by_name.first().map(|n| n.id.clone())
}

pub fn collect_incoming(graph: &Graph, start_id: &NodeId) -> Vec<RelatedNodeOutput> {
    let mut out: Vec<RelatedNodeOutput> = Vec::new();
    for edge in graph.edges_to(start_id) {
        if let Some(src) = graph.get_node(&edge.source) {
            out.push(RelatedNodeOutput {
                name: src.name.clone(),
                label: src.label.to_string(),
                qualified_name: src.qualified_name.clone(),
                file_path: src.file_path.clone(),
                start_line: src.start_line,
                edge_type: edge.edge_type.to_string(),
                edge_confidence: edge.confidence,
                edge_reason: edge.reason.clone(),
            });
        }
    }
    out.sort_by(|a, b| {
        a.edge_type
            .cmp(&b.edge_type)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

pub fn collect_outgoing(graph: &Graph, start_id: &NodeId) -> Vec<RelatedNodeOutput> {
    let mut out: Vec<RelatedNodeOutput> = Vec::new();
    for edge in graph.edges_from(start_id) {
        if let Some(dst) = graph.get_node(&edge.target) {
            out.push(RelatedNodeOutput {
                name: dst.name.clone(),
                label: dst.label.to_string(),
                qualified_name: dst.qualified_name.clone(),
                file_path: dst.file_path.clone(),
                start_line: dst.start_line,
                edge_type: edge.edge_type.to_string(),
                edge_confidence: edge.confidence,
                edge_reason: edge.reason.clone(),
            });
        }
    }
    out.sort_by(|a, b| {
        a.edge_type
            .cmp(&b.edge_type)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

pub fn collect_processes(graph: &Graph, start_id: &NodeId) -> Vec<RelatedNodeOutput> {
    const PROCESS_EDGE_TYPES: [EdgeType; 4] = [
        EdgeType::StepInProcess,
        EdgeType::EntryPointOf,
        EdgeType::HandlesRoute,
        EdgeType::HandlesTool,
    ];
    let mut out: Vec<RelatedNodeOutput> = Vec::new();
    for edge in graph.edges.iter() {
        if !PROCESS_EDGE_TYPES.contains(&edge.edge_type) {
            continue;
        }
        let other_id = if edge.source == *start_id {
            Some(&edge.target)
        } else if edge.target == *start_id {
            Some(&edge.source)
        } else {
            None
        };
        let Some(other_id) = other_id else { continue };
        let Some(other) = graph.get_node(other_id) else {
            continue;
        };
        out.push(RelatedNodeOutput {
            name: other.name.clone(),
            label: other.label.to_string(),
            qualified_name: other.qualified_name.clone(),
            file_path: other.file_path.clone(),
            start_line: other.start_line,
            edge_type: edge.edge_type.to_string(),
            edge_confidence: edge.confidence,
            edge_reason: edge.reason.clone(),
        });
    }
    out.sort_by(|a, b| {
        a.edge_type
            .cmp(&b.edge_type)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Edge, EdgeType, Language, Node, NodeLabel};

    fn make_node(id: &str, name: &str, qn: &str, label: NodeLabel, file: &str, line: u32) -> Node {
        Node::builder(label, name, qn)
            .id(id)
            .file_path(file)
            .start_line(line)
            .end_line(line + 5)
            .language(Language::Rust)
            .signature(format!("fn {name}()"))
            .is_exported(true)
            .build()
    }

    #[test]
    fn resolve_start_id_by_name() {
        let mut graph = Graph::new();
        graph.add_node(make_node(
            "id1",
            "foo",
            "demo.foo",
            NodeLabel::Function,
            "/x.rs",
            1,
        ));
        assert_eq!(resolve_start_id(&graph, "foo").as_deref(), Some("id1"));
    }

    #[test]
    fn resolve_start_id_by_qualified_name() {
        let mut graph = Graph::new();
        graph.add_node(make_node(
            "id1",
            "foo",
            "demo.foo",
            NodeLabel::Function,
            "/x.rs",
            1,
        ));
        assert_eq!(resolve_start_id(&graph, "demo.foo").as_deref(), Some("id1"));
    }

    #[test]
    fn resolve_start_id_missing_returns_none() {
        let graph = Graph::new();
        assert!(resolve_start_id(&graph, "missing").is_none());
    }

    #[test]
    fn collect_incoming_returns_callers() {
        let mut graph = Graph::new();
        graph.add_node(make_node(
            "a",
            "a",
            "demo.a",
            NodeLabel::Function,
            "/a.rs",
            1,
        ));
        graph.add_node(make_node(
            "b",
            "b",
            "demo.b",
            NodeLabel::Function,
            "/b.rs",
            1,
        ));
        graph.add_edge(Edge::new("a", "b", EdgeType::Calls, "demo"));
        let incoming = collect_incoming(&graph, &"b".to_string());
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].name, "a");
        assert_eq!(incoming[0].edge_type, "CALLS");
    }

    #[test]
    fn collect_outgoing_returns_callees() {
        let mut graph = Graph::new();
        graph.add_node(make_node(
            "a",
            "a",
            "demo.a",
            NodeLabel::Function,
            "/a.rs",
            1,
        ));
        graph.add_node(make_node(
            "b",
            "b",
            "demo.b",
            NodeLabel::Function,
            "/b.rs",
            1,
        ));
        graph.add_edge(Edge::new("a", "b", EdgeType::Calls, "demo"));
        let outgoing = collect_outgoing(&graph, &"a".to_string());
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].name, "b");
        assert_eq!(outgoing[0].edge_type, "CALLS");
    }

    #[test]
    fn collect_processes_finds_step_in_process() {
        let mut graph = Graph::new();
        graph.add_node(make_node(
            "a",
            "a",
            "demo.a",
            NodeLabel::Function,
            "/a.rs",
            1,
        ));
        graph.add_node(
            Node::builder(NodeLabel::Process, "checkout", "demo.checkout")
                .id("p1")
                .build(),
        );
        graph.add_edge(Edge::new("a", "p1", EdgeType::StepInProcess, "demo"));
        let processes = collect_processes(&graph, &"a".to_string());
        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].name, "checkout");
        assert_eq!(processes[0].edge_type, "STEP_IN_PROCESS");
    }

    #[test]
    fn collect_processes_finds_entry_point_of() {
        let mut graph = Graph::new();
        graph.add_node(make_node(
            "main",
            "main",
            "demo.main",
            NodeLabel::Function,
            "/m.rs",
            1,
        ));
        graph.add_node(
            Node::builder(NodeLabel::Process, "bootstrap", "demo.bootstrap")
                .id("p1")
                .build(),
        );
        graph.add_edge(Edge::new("main", "p1", EdgeType::EntryPointOf, "demo"));
        let processes = collect_processes(&graph, &"main".to_string());
        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].name, "bootstrap");
        assert_eq!(processes[0].edge_type, "ENTRY_POINT_OF");
    }

    #[test]
    fn collect_processes_ignores_call_edges() {
        let mut graph = Graph::new();
        graph.add_node(make_node(
            "a",
            "a",
            "demo.a",
            NodeLabel::Function,
            "/a.rs",
            1,
        ));
        graph.add_node(make_node(
            "b",
            "b",
            "demo.b",
            NodeLabel::Function,
            "/b.rs",
            1,
        ));
        graph.add_edge(Edge::new("a", "b", EdgeType::Calls, "demo"));
        let processes = collect_processes(&graph, &"a".to_string());
        assert!(processes.is_empty());
    }

    #[test]
    fn collect_incoming_sorts_by_edge_type_then_name() {
        let mut graph = Graph::new();
        graph.add_node(make_node("target", "target", "demo.target", NodeLabel::Function, "/t.rs", 1));
        graph.add_node(make_node("c1", "z_caller", "demo.z_caller", NodeLabel::Function, "/z.rs", 1));
        graph.add_node(make_node("c2", "a_caller", "demo.a_caller", NodeLabel::Function, "/a.rs", 1));
        graph.add_node(make_node("c3", "m_caller", "demo.m_caller", NodeLabel::Function, "/m.rs", 1));
        graph.add_edge(Edge::new("c1", "target", EdgeType::DataFlows, "demo"));
        graph.add_edge(Edge::new("c2", "target", EdgeType::Calls, "demo"));
        graph.add_edge(Edge::new("c3", "target", EdgeType::Calls, "demo"));
        let incoming = collect_incoming(&graph, &"target".to_string());
        assert_eq!(incoming.len(), 3);
        // Sort: edge_type asc, then name asc → CALLS before DATAFLOWS,
        // and within CALLS: a_caller before m_caller.
        assert_eq!(incoming[0].edge_type, "CALLS");
        assert_eq!(incoming[0].name, "a_caller");
        assert_eq!(incoming[1].edge_type, "CALLS");
        assert_eq!(incoming[1].name, "m_caller");
        assert_eq!(incoming[2].edge_type, "DATAFLOWS");
        assert_eq!(incoming[2].name, "z_caller");
    }

    #[test]
    fn collect_outgoing_sorts_by_edge_type_then_name() {
        let mut graph = Graph::new();
        graph.add_node(make_node("src", "src", "demo.src", NodeLabel::Function, "/s.rs", 1));
        graph.add_node(make_node("d1", "z_callee", "demo.z_callee", NodeLabel::Function, "/z.rs", 1));
        graph.add_node(make_node("d2", "a_callee", "demo.a_callee", NodeLabel::Function, "/a.rs", 1));
        graph.add_edge(Edge::new("src", "d1", EdgeType::DataFlows, "demo"));
        graph.add_edge(Edge::new("src", "d2", EdgeType::Calls, "demo"));
        let outgoing = collect_outgoing(&graph, &"src".to_string());
        assert_eq!(outgoing.len(), 2);
        // CALLS before DATAFLOWS.
        assert_eq!(outgoing[0].edge_type, "CALLS");
        assert_eq!(outgoing[0].name, "a_callee");
        assert_eq!(outgoing[1].edge_type, "DATAFLOWS");
        assert_eq!(outgoing[1].name, "z_callee");
    }

    #[test]
    fn collect_processes_finds_start_as_target() {
        let mut graph = Graph::new();
        graph.add_node(make_node("handler", "handler", "demo.handler", NodeLabel::Function, "/h.rs", 1));
        graph.add_node(
            Node::builder(NodeLabel::Process, "checkout", "demo.checkout")
                .id("p1")
                .build(),
        );
        // Edge from process TO start node (start is the target).
        graph.add_edge(Edge::new("p1", "handler", EdgeType::HandlesRoute, "demo"));
        let processes = collect_processes(&graph, &"handler".to_string());
        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].name, "checkout");
        assert_eq!(processes[0].edge_type, "HANDLES_ROUTE");
    }

    #[test]
    fn collect_processes_sorts_multiple() {
        let mut graph = Graph::new();
        graph.add_node(make_node("handler", "handler", "demo.handler", NodeLabel::Function, "/h.rs", 1));
        graph.add_node(
            Node::builder(NodeLabel::Process, "z_process", "demo.z_process")
                .id("p1")
                .build(),
        );
        graph.add_node(
            Node::builder(NodeLabel::Process, "a_process", "demo.a_process")
                .id("p2")
                .build(),
        );
        graph.add_node(
            Node::builder(NodeLabel::Process, "m_process", "demo.m_process")
                .id("p3")
                .build(),
        );
        // Mix of StepInProcess and HandlesTool edges.
        graph.add_edge(Edge::new("handler", "p1", EdgeType::StepInProcess, "demo"));
        graph.add_edge(Edge::new("handler", "p2", EdgeType::HandlesTool, "demo"));
        graph.add_edge(Edge::new("p3", "handler", EdgeType::EntryPointOf, "demo"));
        let processes = collect_processes(&graph, &"handler".to_string());
        assert_eq!(processes.len(), 3);
        // Sort: edge_type asc → ENTRY_POINT_OF, HANDLES_TOOL, STEP_IN_PROCESS.
        assert_eq!(processes[0].edge_type, "ENTRY_POINT_OF");
        assert_eq!(processes[0].name, "m_process");
        assert_eq!(processes[1].edge_type, "HANDLES_TOOL");
        assert_eq!(processes[1].name, "a_process");
        assert_eq!(processes[2].edge_type, "STEP_IN_PROCESS");
        assert_eq!(processes[2].name, "z_process");
    }
}
