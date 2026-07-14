// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Graph adjacency-index benchmarks (MED-002).
//!
//! Measures `edges_from` / `edges_to` / `neighbors` latency on a large
//! in-memory graph. Before MED-002 these were O(E) full scans; the
//! adjacency index makes them O(deg(n)).
//!
//! Graph shape: N nodes, each with K outgoing edges to the next K nodes
//! (wrapping). Total edges = N * K. A BFS traversal exercises
//! `edges_from` across all reachable nodes.

use codenexus::model::{Edge, EdgeType, Graph, Node, NodeLabel};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

const NODE_COUNT: usize = 10_000;
const EDGE_PER_NODE: usize = 5;

/// Builds a graph with `nodes` functions, each having `fanout` outgoing
/// Calls edges to the next `fanout` nodes (wrapping around).
fn build_large_graph(nodes: usize, fanout: usize) -> Graph {
    let mut g = Graph::new();
    for i in 0..nodes {
        let name = format!("func_{i}");
        let node = Node::builder(NodeLabel::Function, name.clone(), format!("bench.{name}"))
            .id(format!("f{i}"))
            .project("bench")
            .file_path(format!("src/{name}.rs"))
            .start_line(10)
            .build();
        g.add_node(node);
    }
    for i in 0..nodes {
        for j in 1..=fanout {
            let target = (i + j) % nodes;
            g.add_edge(Edge::new(
                format!("f{i}"),
                format!("f{target}"),
                EdgeType::Calls,
                "bench",
            ));
        }
    }
    g
}

/// BFS traversal calling `edges_from` for every visited node — simulates
/// the real trace/context hot path.
fn bfs_edges_from(g: &Graph, start: &str) -> usize {
    let mut visited = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(start.to_string());
    visited.insert(start.to_string());
    let mut edge_visits = 0;
    while let Some(id) = queue.pop_front() {
        for edge in g.edges_from(&id) {
            edge_visits += 1;
            if visited.insert(edge.target.clone()) {
                queue.push_back(edge.target.clone());
            }
        }
    }
    edge_visits
}

fn bench_edges_from(c: &mut Criterion) {
    let graph = build_large_graph(NODE_COUNT, EDGE_PER_NODE);

    let mut group = c.benchmark_group("graph_edges_from");
    group.sample_size(50);

    // Single-node hotspot: one edges_from call on a node with K outgoing
    // edges, but the graph has N*K total edges. Pre-MED-002 this scans
    // all N*K edges every call.
    group.bench_function("single_node", |b| {
        b.iter(|| {
            let edges = graph.edges_from(&"f0".to_string());
            black_box(edges);
        });
    });

    // BFS traversal: exercises edges_from across all reachable nodes.
    group.bench_function("bfs_traversal", |b| {
        b.iter(|| {
            let visits = bfs_edges_from(&graph, "f0");
            black_box(visits);
        });
    });

    group.finish();
}

fn bench_edges_to(c: &mut Criterion) {
    let graph = build_large_graph(NODE_COUNT, EDGE_PER_NODE);

    let mut group = c.benchmark_group("graph_edges_to");
    group.sample_size(50);

    group.bench_function("single_node", |b| {
        b.iter(|| {
            let edges = graph.edges_to(&"f0".to_string());
            black_box(edges);
        });
    });

    group.finish();
}

fn bench_neighbors(c: &mut Criterion) {
    let graph = build_large_graph(NODE_COUNT, EDGE_PER_NODE);

    let mut group = c.benchmark_group("graph_neighbors");
    group.sample_size(50);

    group.bench_function("unfiltered", |b| {
        b.iter(|| {
            let ns = graph.neighbors(&"f0".to_string(), None);
            black_box(ns);
        });
    });

    group.bench_function("filtered_by_type", |b| {
        b.iter(|| {
            let ns = graph.neighbors(&"f0".to_string(), Some(EdgeType::Calls));
            black_box(ns);
        });
    });

    group.finish();
}

fn bench_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph_scaling");
    group.sample_size(30);

    for &nodes in &[1_000usize, 5_000, 10_000] {
        let fanout = 5;
        let graph = build_large_graph(nodes, fanout);
        group.bench_with_input(
            BenchmarkId::new("edges_from_single", nodes),
            &nodes,
            |b, _| {
                b.iter(|| {
                    let edges = graph.edges_from(&"f0".to_string());
                    black_box(edges);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_edges_from, bench_edges_to, bench_neighbors, bench_scaling);
criterion_main!(benches);
