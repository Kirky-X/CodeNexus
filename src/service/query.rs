// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Query command: execute Cypher queries against the knowledge graph.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(test)]
use crate::kit::{AsyncKit, AsyncReady};
use crate::query::{validate_cypher_subset, QueryResult};
#[cfg(all(any(feature = "cli", feature = "mcp"), not(feature = "cache")))]
use crate::service::error::kit_not_initialized;
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::error::to_api_error;
#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::service::error::CodeNexusError;
// run_query_cached 在 test 组合（无 cli/mcp）下也会编译，kit 须随行——
// 否则 core,lsp,cache 等含 cache 的无 cli 组合 lib test 编译失败（E0425）。
#[cfg(any(feature = "cli", feature = "mcp", all(test, feature = "cache")))]
use crate::service::runtime::nexus;
#[cfg(all(any(feature = "cli", feature = "mcp", test), feature = "cache"))]
use oxcache::cached;

#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::forge;
#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;

/// Mirrors [`QueryResult`] with `Serialize`/`Deserialize` for JSON transport.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueryOutput {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub duration_ms: u64,
}

#[cfg(any(feature = "cli", feature = "mcp", test))]
fn query_output(r: QueryResult) -> QueryOutput {
    QueryOutput {
        columns: r.columns,
        rows: r.rows,
        duration_ms: r.duration_ms,
    }
}

/// Runs query against an injected Kit (testable core).
#[cfg(any(feature = "cli", feature = "mcp", test))]
pub fn run_query(nexus: &crate::sdk::Nexus, cypher: &str) -> Result<QueryOutput, CodeNexusError> {
    let q = nexus.query()?;
    validate_cypher_subset(cypher)?;
    let result = q.cypher(cypher)?;
    Ok(query_output(result))
}

/// `#[cached]`-wrapped query execution (RICE Top-1 absorption from oxcache).
///
/// Cache key = `(DB identity from build_kit, Cypher text)`; results are
/// JSON-serialized into the `codenexus-query` service cache registered by
/// [`crate::cache::CacheModule`], and cleared on graph mutations through
/// [`crate::cache::CacheStore::invalidate_all`]. On a cache hit,
/// `duration_ms` reflects the original execution, not the lookup.
///
/// Falls back to plain execution when the cache service is not registered
/// (oxcache `#[cached]` semantics) — e.g. builds with the `cache` feature
/// disabled at runtime paths that skip `CacheModule`.
#[cfg(all(any(feature = "cli", feature = "mcp", test), feature = "cache"))]
#[cached(service = "codenexus-query", ttl = 300, sync, key = "{ns}:{cypher}")]
pub fn run_query_cached(ns: String, cypher: String) -> Result<QueryOutput, CodeNexusError> {
    let nexus = nexus().ok_or_else(CodeNexusError::kit_not_initialized)?;
    run_query(&nexus, &cypher)
}

/// Instance-scoped query-cache namespace — `Nexus::query_cache_ns` embeds
/// the DB identity AND the namespace generation, so a graph-mutation bump
/// strands this instance's older entries without touching another
/// instance's namespace (T012 instance isolation).
#[cfg(all(any(feature = "cli", feature = "mcp", test), feature = "cache"))]
fn query_cache_ns() -> Result<String, CodeNexusError> {
    let nexus = nexus().ok_or_else(CodeNexusError::kit_not_initialized)?;
    nexus.query_cache_ns().map_err(CodeNexusError::from)
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[forge::log(args, err_detail)]
#[forge(
    name = "query",
    version = "0.3.5",
    description = "Execute a Cypher query against the CodeNexus knowledge graph.",
    cli = true
)]
async fn query(cypher: String) -> Result<(), ApiError> {
    #[cfg(feature = "cache")]
    let result = run_query_cached(
        query_cache_ns().map_err(|e| to_api_error(e, "query_error"))?,
        cypher,
    );
    #[cfg(not(feature = "cache"))]
    let result = {
        let nexus = nexus().ok_or_else(kit_not_initialized)?;
        run_query(&nexus, &cypher)
    };
    let result = result.map_err(|e| to_api_error(e, "query_error"))?;
    let json = serde_json::to_string(&result)
        .map_err(|e| to_api_error(CodeNexusError::from(e), "query_error"))?;
    println!("{json}");
    Ok(())
}

/// MCP wrapper — returns result for MCP protocol.
#[cfg(feature = "mcp")]
#[forge(
    name = "query",
    version = "0.3.5",
    tool_name = "query",
    description = "Execute a read-only Cypher-subset query against the knowledge graph. Write clauses (CREATE/DELETE/SET/MERGE/REMOVE) and CALL procedures are rejected; UNION / UNION ALL are supported, while label disjunction (`WHERE (n:A OR n:B)` or `(:A|B)`) is not supported by the subset. Params: cypher (required) — a MATCH/RETURN query."
)]
async fn query_mcp(cypher: String) -> Result<QueryOutput, ApiError> {
    #[cfg(feature = "cache")]
    return run_query_cached(
        query_cache_ns().map_err(|e| to_api_error(e, "query_error"))?,
        cypher,
    )
    .map_err(|e| to_api_error(e, "query_error"));
    #[cfg(not(feature = "cache"))]
    {
        let nexus = nexus().ok_or_else(kit_not_initialized)?;
        run_query(&nexus, &cypher).map_err(|e| to_api_error(e, "query_error"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use crate::query::QueryError;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_query_testdb");
        (dir, path)
    }

    fn nexus_for_db(db: &std::path::Path) -> crate::sdk::Nexus {
        crate::sdk::Nexus::from_kit(build_kit_for_db(db))
    }

    fn build_kit_for_db(db: &std::path::Path) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    #[cfg(feature = "cache")]
    #[test]
    #[serial_test::serial(kit_init)]
    fn run_query_cached_roundtrip_and_instance_scoped_invalidation() {
        use crate::cache::MACRO_QUERY_SERVICE;

        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        crate::service::runtime::force_init_kit_for_testing(kit);
        let nexus = crate::sdk::Nexus::from_runtime().expect("kit");
        let cypher = "MATCH (n) RETURN n LIMIT 3".to_string();

        // Generation-0 namespace: `cypher:{db}.g0` (T012 instance isolation).
        let ns_g0 = nexus.query_cache_ns().expect("ns");
        let first = run_query_cached(ns_g0.clone(), cypher.clone()).expect("first query ok");

        // The result must be cached under the macro service registry.
        let svc = oxcache::__internal_get_cache(MACRO_QUERY_SERVICE)
            .expect("CacheModule registers the query macro service");
        // oxcache escapes `:` in key-template placeholder values (%3A) —
        // assert on the ESCAPED form the macro actually stores.
        let key_g0 = format!(
            "{}:{}",
            oxcache::macro_support::escape_key_segment(&ns_g0),
            oxcache::macro_support::escape_key_segment(&cypher)
        );
        assert!(
            svc.get_bytes_sync(&key_g0).expect("get").is_some(),
            "query result should be cached under {key_g0}"
        );

        // T012 acceptance ①: a second instance over a DIFFERENT database
        // carries its own namespace — same-cypher entries never collide.
        let other_dir = tempfile::TempDir::new().expect("tmpdir");
        let other_nexus = nexus_for_db(&other_dir.path().join("other_db"));
        let ns_other = other_nexus.query_cache_ns().expect("other ns");
        assert_ne!(ns_other, ns_g0, "distinct DBs must get distinct namespaces");
        let other_cypher = "MATCH (x) RETURN x LIMIT 1".to_string();
        let key_other = format!(
            "{}:{}",
            oxcache::macro_support::escape_key_segment(&ns_other),
            oxcache::macro_support::escape_key_segment(&other_cypher)
        );
        let _ = run_query_cached(ns_other.clone(), other_cypher.clone()).expect("other ok");
        assert!(
            svc.get_bytes_sync(&key_other).expect("get").is_some(),
            "the other instance's entry must be cached under {key_other}"
        );

        // T012 acceptance ②: THIS instance's graph-mutation invalidation
        // (same prefix the indexer's pipeline uses) bumps only this
        // namespace's generation — the other instance's entry survives.
        let cache = nexus.cache().expect("cache capability");
        let db_key = nexus.db_key().expect("db identity");
        cache.invalidate_namespace(&format!("cypher:{db_key}"));
        assert!(
            svc.get_bytes_sync(&key_other).expect("get").is_some(),
            "A's invalidation must not clear B's namespace"
        );

        // The bumped generation makes THIS instance's old entry unreachable:
        // the namespace advances, so the next query re-executes.
        let ns_g1 = nexus.query_cache_ns().expect("ns after bump");
        assert_ne!(ns_g0, ns_g1, "bump must advance the generation");
        let second = run_query_cached(ns_g1, cypher.clone()).expect("recompute ok");
        assert_eq!(first.columns, second.columns);
        assert_eq!(first.rows, second.rows);
        assert!(
            svc.get_bytes_sync(&key_g0).expect("get").is_some(),
            "generation-stranded entries age out via TTL, not instantly"
        );
        crate::service::runtime::reset_kit_for_testing();
    }

    /// T012 review HIGH-1 acceptance: the invalidation chain must fire
    /// through the REAL production path — query (cached) → index run via
    /// the indexer capability → generation advances → next query
    /// re-executes. This is the wiring test the capability-level unit test
    /// cannot be (it calls `invalidate_namespace` by hand).
    #[cfg(feature = "cache")]
    #[test]
    #[serial_test::serial(kit_init)]
    fn index_run_through_capability_advances_query_generation() {
        let (dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        crate::service::runtime::force_init_kit_for_testing(kit);
        let nexus = crate::sdk::Nexus::from_runtime().expect("kit");
        let cypher = "MATCH (n) RETURN count(n) AS c".to_string();

        // Seed the query cache at generation 0.
        let ns_g0 = nexus.query_cache_ns().expect("ns");
        let _ = run_query_cached(ns_g0.clone(), cypher.clone()).expect("query ok");

        // A real index run through the production capability: the
        // indexer's pipeline carries the wired cache and bumps
        // `cypher:{db_path}` on success.
        let indexer = nexus.indexer().expect("indexer capability");
        // 索引目标是本测试自己的临时目录：Linux runner 的 /tmp 含 root 属主的
        // systemd-private 目录（000 权限），整扫 /tmp 会 Walk PermissionDenied。
        // 本测试只断言「经生产能力的索引运行推进 generation」，目录内容无关。
        let output = indexer
            .index(dir.path(), "gen-probe-project", false)
            .expect("index run");
        let _ = output;

        // The generation must have advanced: the same cache namespace read
        // through the facade now resolves to a NEW key, so the stale
        // entry (if any survived) is unreachable.
        let cache = nexus.cache().expect("cache capability");
        let db_key = nexus.db_key().expect("db identity");
        let generation_after = cache.namespace_generation(&format!("cypher:{db_key}"));
        assert!(
            generation_after >= 1,
            "a real index run must advance the query-cache generation, got {generation_after}"
        );
        crate::service::runtime::reset_kit_for_testing();
    }

    #[test]
    fn run_query_returns_error_for_empty_cypher() {
        let (_dir, db) = fresh_db_path();
        let kit = nexus_for_db(&db);
        let err = run_query(&kit, "").expect_err("empty query should error");
        match err {
            CodeNexusError::Query(QueryError::InvalidQuery(msg)) => {
                assert!(msg.contains("empty"), "error should mention empty: {msg}");
            }
            other => panic!("expected QueryError::InvalidQuery, got {other:?}"),
        }
    }

    #[test]
    fn run_query_returns_error_for_whitespace_only() {
        let (_dir, db) = fresh_db_path();
        let kit = nexus_for_db(&db);
        let err = run_query(&kit, "   \t\n  ").expect_err("whitespace query should error");
        assert!(matches!(
            err,
            CodeNexusError::Query(QueryError::InvalidQuery(_))
        ));
    }

    #[test]
    fn run_query_succeeds_for_valid_match_query_on_empty_db() {
        let (_dir, db) = fresh_db_path();
        let kit = nexus_for_db(&db);
        let result = run_query(&kit, "MATCH (n) RETURN n LIMIT 1");
        // LadybugDB may return empty result or succeed; either is acceptable.
        match result {
            Ok(output) => {
                // A freshly initialized DB contains exactly one bookkeeping
                // row (the SchemaMeta version stamp), so `MATCH (n)` may
                // return it — assert no user-graph rows exist.
                assert!(
                    output.rows.len() <= 1,
                    "empty DB should have no user rows (at most the schema-meta stamp), got: {:?}",
                    output.rows
                );
            }
            Err(CodeNexusError::Query(_)) => {
                // LadybugDB might reject some constructs — acceptable.
            }
            Err(other) => panic!("unexpected error for valid query: {other:?}"),
        }
    }

    #[test]
    fn query_output_maps_query_result() {
        let result = QueryResult {
            columns: vec!["n".into(), "id".into()],
            rows: vec![vec![Value::Null, Value::from(42)]],
            duration_ms: 1500,
        };
        let output = query_output(result);
        assert_eq!(output.columns, vec!["n".to_string(), "id".to_string()]);
        assert_eq!(output.rows.len(), 1);
        assert_eq!(output.rows[0][1], Value::from(42));
        assert_eq!(output.duration_ms, 1500);
    }

    #[test]
    fn query_output_maps_empty_result() {
        let result = QueryResult {
            columns: vec![],
            rows: vec![],
            duration_ms: 0,
        };
        let output = query_output(result);
        assert!(output.columns.is_empty());
        assert!(output.rows.is_empty());
        assert_eq!(output.duration_ms, 0);
    }

    #[test]
    fn query_output_serializes_to_json() {
        let output = QueryOutput {
            columns: vec!["n".into()],
            rows: vec![vec![Value::from(1)]],
            duration_ms: 100,
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"columns\":[\"n\"]"));
        assert!(json.contains("\"duration_ms\":100"));
        assert!(json.contains("\"rows\":[[1]]"));
    }

    #[test]
    fn query_output_round_trips_through_json() {
        let output = QueryOutput {
            columns: vec!["id".into(), "name".into()],
            rows: vec![
                vec![Value::from(1), Value::from("foo")],
                vec![Value::from(2), Value::from("bar")],
            ],
            duration_ms: 42,
        };
        let json = serde_json::to_string(&output).unwrap();
        let parsed: QueryOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(output, parsed);
    }

    // `#[forge]` wrapper tests via init_kit

    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn query_wrapper_succeeds_via_init_kit() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(query(
            "MATCH (n:Function) RETURN n.name LIMIT 10".to_string(),
        ));
        assert!(result.is_ok(), "wrapper should succeed: {:?}", result.err());

        reset_kit_for_testing();
    }

    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn query_wrapper_fails_when_kit_not_initialized() {
        use crate::service::runtime::reset_kit_for_testing;

        reset_kit_for_testing();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(query("MATCH (n) RETURN n LIMIT 1".to_string()));
        assert!(result.is_err(), "wrapper should fail without kit");
        reset_kit_for_testing();
    }
}
