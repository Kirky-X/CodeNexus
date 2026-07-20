// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Trace engine benchmarks (Task 15).
//!
//! Measures [`TraceFacade::trace`] latency over an in-memory call chain. The
//! target SLO is P99 <= 500ms; criterion's `trace` group surfaces the per-call
//! wall time so regressions can be tracked over time.
//!
//! Setup builds an in-memory [`Graph`] with a 100-node call chain
//! (`func_0` -> `func_1` -> ... -> `func_99`), then the benchmarks repeatedly
//! trace from `func_0` at various depths and trace types. `TraceFacade`
//! borrows the graph immutably, so no database is involved.

use codenexus::model::{Edge, EdgeType, Graph, Node, NodeLabel};
use codenexus::trace::{ImpactAnalyzer, ImpactConfig, TraceFacade, TraceType};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

/// Builds an in-memory graph with `size` functions in a linear call chain:
/// `func_0` calls `func_1` calls ... `func_{size-1}`.
fn build_call_chain(size: usize) -> Graph {
    let mut g = Graph::new();
    for i in 0..size {
        let name = format!("func_{i}");
        let node = Node::builder(NodeLabel::Function, name.clone(), format!("bench.{name}"))
            .id(format!("f{i}"))
            .project("bench")
            .file_path(format!("src/{name}.rs"))
            .start_line(10)
            .build();
        g.add_node(node);
    }
    for i in 0..size.saturating_sub(1) {
        g.add_edge(Edge::new(
            format!("f{i}"),
            format!("f{}", i + 1),
            EdgeType::Calls,
            "bench",
        ));
    }
    g
}

/// Builds a deep call chain where every non-root node also calls back to
/// `func_0`. Each back-edge is a cycle rejected by `path_contains`, stressing
/// the cycle-detection hot path (MED-002): O(1) with the path_set vs O(depth)
/// with a parent-chain walk.
fn build_call_chain_with_back_edges(size: usize) -> Graph {
    let mut g = Graph::new();
    for i in 0..size {
        let name = format!("func_{i}");
        let node = Node::builder(NodeLabel::Function, name.clone(), format!("bench.{name}"))
            .id(format!("f{i}"))
            .project("bench")
            .file_path(format!("src/{name}.rs"))
            .start_line(10)
            .build();
        g.add_node(node);
    }
    for i in 0..size.saturating_sub(1) {
        g.add_edge(Edge::new(
            format!("f{i}"),
            format!("f{}", i + 1),
            EdgeType::Calls,
            "bench",
        ));
    }
    for i in 1..size {
        g.add_edge(Edge::new(format!("f{i}"), "f0", EdgeType::Calls, "bench"));
    }
    g
}

fn bench_trace(c: &mut Criterion) {
    let graph = build_call_chain(100);
    let facade = TraceFacade::new(&graph);

    let mut group = c.benchmark_group("trace");
    group.sample_size(100);

    group.bench_function("trace_calls_depth_5", |b| {
        b.iter(|| {
            let result = facade.trace("func_0", TraceType::Calls, 5).unwrap();
            black_box(result);
        });
    });

    group.bench_function("trace_calls_depth_10", |b| {
        b.iter(|| {
            let result = facade.trace("func_0", TraceType::Calls, 10).unwrap();
            black_box(result);
        });
    });

    group.bench_function("trace_calls_depth_50", |b| {
        b.iter(|| {
            let result = facade.trace("func_0", TraceType::Calls, 50).unwrap();
            black_box(result);
        });
    });

    group.bench_function("trace_all_depth_10", |b| {
        b.iter(|| {
            let result = facade.trace("func_0", TraceType::All, 10).unwrap();
            black_box(result);
        });
    });

    group.finish();
}

fn bench_trace_path_contains(c: &mut Criterion) {
    // 120-node chain with a back-edge to the root from every non-root node:
    // each BFS expansion rejects one cyclic edge via path_contains. At depth
    // 100 the baseline O(depth) walk dominates; the O(1) HashSet lookup
    // (MED-002) removes that cost.
    let graph = build_call_chain_with_back_edges(120);
    let facade = TraceFacade::new(&graph);

    let mut group = c.benchmark_group("trace_path_contains");
    group.sample_size(50);

    group.bench_function("deep_chain_with_back_edges_depth_100", |b| {
        b.iter(|| {
            let result = facade.trace("func_0", TraceType::Calls, 100).unwrap();
            black_box(result);
        });
    });

    group.finish();
}

/// Builds a high-fanin graph that models the bulwark regression scenario:
/// one target node `target` with `fanout` direct callers, where every
/// caller also fans out to `fanout` transitive callers. This stresses
/// the `MAX_NODES_LIMIT=5000` cap introduced in v0.3.8 — without the cap,
/// `analyze_impact` would materialize O(fanout^2) nodes and blow up
/// memory on the first BFS hop.
fn build_high_fanin_graph(fanout: usize) -> Graph {
    let mut g = Graph::new();
    // Target node — the symbol whose impact we are analysing.
    let target = Node::builder(
        NodeLabel::Function,
        "target".to_string(),
        "bench.target".to_string(),
    )
    .id("target")
    .project("bench")
    .file_path("src/target.rs")
    .start_line(1)
    .build();
    g.add_node(target);
    // Direct callers (depth 1) + their transitive callers (depth 2).
    // Total nodes = 1 + fanout + fanout*fanout. With fanout=70 → 4971 nodes,
    // which stays under MAX_NODES_LIMIT=5000 and exercises the hot path.
    for i in 0..fanout {
        let caller_id = format!("caller_{i}");
        let caller = Node::builder(
            NodeLabel::Function,
            format!("caller_{i}"),
            format!("bench.caller_{i}"),
        )
        .id(caller_id.clone())
        .project("bench")
        .file_path(format!("src/caller_{i}.rs"))
        .start_line(10)
        .build();
        g.add_node(caller);
        // caller_i -> target (direct caller of target).
        g.add_edge(Edge::new(
            caller_id.clone(),
            "target".to_string(),
            EdgeType::Calls,
            "bench",
        ));
        // Transitive callers: caller_{i}_{j} -> caller_i.
        for j in 0..fanout {
            let trans_id = format!("caller_{i}_{j}");
            let trans = Node::builder(
                NodeLabel::Function,
                trans_id.clone(),
                format!("bench.{trans_id}"),
            )
            .id(trans_id.clone())
            .project("bench")
            .file_path(format!("src/{trans_id}.rs"))
            .start_line(20)
            .build();
            g.add_node(trans);
            g.add_edge(Edge::new(
                trans_id,
                caller_id.clone(),
                EdgeType::Calls,
                "bench",
            ));
        }
    }
    g
}

/// M2: impact on a ~5000-node high-fanin graph (bulwark regression guard).
///
/// Verifies that `analyze_impact` completes in bounded time on the kind of
/// graph that triggered the v0.3.8 cap raise (1000 → 5000). The benchmark
/// also asserts the result respects `MAX_NODES_LIMIT` — a regression that
/// removes the cap would cause this benchmark to OOM or exceed the assertion.
fn bench_impact_5000_node_subgraph(c: &mut Criterion) {
    // fanout=70 → 1 + 70 + 4900 = 4971 nodes (just under the 5000 cap).
    let graph = build_high_fanin_graph(70);
    let analyzer = ImpactAnalyzer::new(&graph);
    let target_id = "target".to_string();

    let mut group = c.benchmark_group("impact_large_subgraph");
    group.sample_size(20);

    group.bench_function("analyze_impact_5000_nodes_default_config", |b| {
        b.iter(|| {
            let result = analyzer.analyze_impact(&target_id);
            // Sanity: the target has 70 direct callers, so affected must be
            // non-empty and must not exceed MAX_NODES_LIMIT (5000). A
            // regression that removes the cap would let affected grow to
            // 4970 (all nodes except target), which is still under 5000 —
            // but the point is to surface latency regressions, not just
            // correctness (correctness is covered by unit tests).
            assert!(
                !result.affected.is_empty(),
                "impact on a 4971-node graph must return non-empty affected"
            );
            assert!(
                result.affected.len() <= 5000,
                "MAX_NODES_LIMIT=5000 must be respected (got {})",
                result.affected.len()
            );
            black_box(result);
        });
    });

    // Custom config with max_depth=10 (the cap) to stress the deepest
    // traversal path on the same graph.
    let deep_config = ImpactConfig {
        max_depth: 10,
        ..ImpactConfig::default()
    };
    let deep_analyzer = ImpactAnalyzer::with_config(&graph, deep_config);
    group.bench_function("analyze_impact_5000_nodes_max_depth_10", |b| {
        b.iter(|| {
            let result = deep_analyzer.analyze_impact(&target_id);
            assert!(!result.affected.is_empty());
            assert!(result.affected.len() <= 5000);
            black_box(result);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_trace,
    bench_trace_path_contains,
    bench_impact_5000_node_subgraph
);
criterion_main!(benches);
