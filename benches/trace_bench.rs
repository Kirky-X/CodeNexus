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
use codenexus::trace::{TraceFacade, TraceType};
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

criterion_group!(benches, bench_trace, bench_trace_path_contains);
criterion_main!(benches);
