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

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use codenexus::model::{Edge, EdgeType, Graph, Node, NodeLabel};
use codenexus::trace::{TraceFacade, TraceType};

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

criterion_group!(benches, bench_trace);
criterion_main!(benches);
