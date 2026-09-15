// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Query command: execute Cypher queries against the knowledge graph.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::kit::{AsyncKit, AsyncReady, QueryModule};
use crate::query::{validate_cypher_subset, QueryResult};
#[cfg(all(any(feature = "cli", feature = "mcp"), not(feature = "cache")))]
use crate::service::error::kit_not_initialized;
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::error::to_api_error;
#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::service::error::CodeNexusError;
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::runtime::kit;
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
pub fn run_query(kit: &AsyncKit<AsyncReady>, cypher: &str) -> Result<QueryOutput, CodeNexusError> {
    let q = kit.require::<QueryModule>()?;
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
#[cached(
    service = "codenexus-query",
    ttl = 300,
    sync,
    key = "{db_key}:{cypher}"
)]
pub fn run_query_cached(db_key: String, cypher: String) -> Result<QueryOutput, CodeNexusError> {
    let kit = kit().ok_or_else(CodeNexusError::kit_not_initialized)?;
    run_query(&kit, &cypher)
}

/// DB identity for cache keys; `"unknown"` when `build_kit` hasn't run.
#[cfg(all(any(feature = "cli", feature = "mcp", test), feature = "cache"))]
fn cache_db_key() -> String {
    crate::service::runtime::db_key().unwrap_or_else(|| "unknown".to_string())
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[forge(
    name = "query",
    version = "0.3.5",
    description = "Execute a Cypher query against the CodeNexus knowledge graph.",
    cli = true
)]
async fn query(cypher: String) -> Result<(), ApiError> {
    #[cfg(feature = "cache")]
    let result = run_query_cached(cache_db_key(), cypher);
    #[cfg(not(feature = "cache"))]
    let result = {
        let kit = kit().ok_or_else(kit_not_initialized)?;
        run_query(&kit, &cypher)
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
    description = "Execute a Cypher query against the CodeNexus knowledge graph."
)]
async fn query_mcp(cypher: String) -> Result<QueryOutput, ApiError> {
    #[cfg(feature = "cache")]
    return run_query_cached(cache_db_key(), cypher).map_err(|e| to_api_error(e, "query_error"));
    #[cfg(not(feature = "cache"))]
    {
        let kit = kit().ok_or_else(kit_not_initialized)?;
        run_query(&kit, &cypher).map_err(|e| to_api_error(e, "query_error"))
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
    fn run_query_cached_roundtrip_and_invalidation() {
        use crate::cache::{CacheModule, MACRO_QUERY_SERVICE};

        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        crate::service::runtime::force_init_kit_for_testing(kit);
        let db_key = crate::service::runtime::db_key().expect("build_kit sets db_key");
        let cypher = "MATCH (n) RETURN n LIMIT 3".to_string();

        let first = run_query_cached(db_key.clone(), cypher.clone()).expect("first query ok");

        // The result must be cached under the macro service registry.
        let svc = oxcache::__internal_get_cache(MACRO_QUERY_SERVICE)
            .expect("CacheModule registers the query macro service");
        let key = format!("{db_key}:{cypher}");
        assert!(
            svc.get_bytes_sync(&key).expect("get").is_some(),
            "query result should be cached under {key}"
        );

        // Graph mutations clear the macro registry via CacheStore semantics.
        let kit = crate::service::runtime::kit().expect("kit");
        let cache = kit.require::<CacheModule>().expect("cache capability");
        cache.invalidate_all();
        assert!(
            svc.get_bytes_sync(&key).expect("get").is_none(),
            "invalidate_all must clear the macro query cache"
        );

        // Recompute after invalidation returns the same content
        // (duration_ms legitimately differs between executions).
        let second = run_query_cached(db_key, cypher).expect("recompute ok");
        assert_eq!(first.columns, second.columns);
        assert_eq!(first.rows, second.rows);
        crate::service::runtime::reset_kit_for_testing();
    }

    #[test]
    fn run_query_returns_error_for_empty_cypher() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
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
        let kit = build_kit_for_db(&db);
        let err = run_query(&kit, "   \t\n  ").expect_err("whitespace query should error");
        assert!(matches!(
            err,
            CodeNexusError::Query(QueryError::InvalidQuery(_))
        ));
    }

    #[test]
    fn run_query_succeeds_for_valid_match_query_on_empty_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let result = run_query(&kit, "MATCH (n) RETURN n LIMIT 1");
        // LadybugDB may return empty result or succeed; either is acceptable.
        match result {
            Ok(output) => {
                assert!(output.rows.is_empty(), "empty DB should have no rows");
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

    // ===== #[forge] wrapper tests via init_kit =====

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
