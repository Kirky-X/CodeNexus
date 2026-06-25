//! Query engine benchmarks (Task 15).
//!
//! Measures [`QueryFacade`] Cypher execution and structured search latency
//! over a pre-indexed fixture. The target SLO is P99 <= 200ms; criterion's
//! `query` group surfaces the per-call wall time so regressions can be
//! tracked over time.
//!
//! Setup indexes 50 Rust files once (each defining two functions with a call
//! edge), then the benchmarks repeatedly execute Cypher queries and structured
//! searches against the resulting graph. The temp dir is intentionally leaked
//! so LadybugDB's open file handles remain valid for the `QueryFacade`'s
//! lifetime (mirrors the storage tests' approach).

use std::path::Path;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use codenexus::index::IndexFacade;
use codenexus::query::QueryFacade;
use tempfile::TempDir;

/// Writes `count` Rust files into `dir`, each defining `func_{i}` which calls
/// `helper_{i}`. This produces call edges the query engine can traverse.
fn write_rust_files(dir: &Path, count: usize) {
    for i in 0..count {
        let path = dir.join(format!("file_{i}.rs"));
        let content = format!(
            "fn func_{i}() {{ helper_{i}(); }}\nfn helper_{i}() {{}}\n"
        );
        std::fs::write(&path, content).unwrap();
    }
}

fn bench_query(c: &mut Criterion) {
    // --- Setup: index a fixture once -------------------------------------
    // The temp dir is leaked so LadybugDB's open handles stay valid for the
    // QueryFacade's lifetime (the facade holds an open connection). The path
    // is captured before `forget` moves the `TempDir`.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    std::mem::forget(tmp);
    write_rust_files(&root, 50);

    let db = root.join("bench.db");
    let facade = IndexFacade::new(&db).unwrap();
    let index_result = facade.index(&root, "bench", false).unwrap();
    let project_id = index_result.project_id;

    let query = QueryFacade::new(&db).unwrap();

    // --- Benchmarks ------------------------------------------------------
    let mut group = c.benchmark_group("query");
    group.sample_size(50);

    group.bench_function("cypher_match_functions", |b| {
        b.iter(|| {
            let result = query
                .cypher("MATCH (f:Function) RETURN f.name AS name LIMIT 10;")
                .unwrap();
            black_box(result);
        });
    });

    group.bench_function("cypher_count_functions", |b| {
        b.iter(|| {
            let result = query
                .cypher("MATCH (f:Function) RETURN count(f) AS cnt;")
                .unwrap();
            black_box(result);
        });
    });

    group.bench_function("search_by_name", |b| {
        b.iter(|| {
            let results = query.search("func", None, 50).unwrap();
            black_box(results);
        });
    });

    group.bench_function("search_by_name_project", |b| {
        b.iter(|| {
            let results = query
                .search("func", Some(project_id.as_str()), 50)
                .unwrap();
            black_box(results);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_query);
criterion_main!(benches);
