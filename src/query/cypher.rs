// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Cypher query executor (PRD §4.4.1).
//!
//! [`CypherExecutor`] wraps a borrowed [`StorageConnection`] and executes raw
//! Cypher queries, returning a [`QueryResult`] with column names, rows, and
//! wall-clock duration. This is the backend for the CLI `query` command.

use std::time::Instant;

use super::error::{QueryError, Result};
use super::QueryResult;
use crate::storage::StorageConnection;

#[cfg(feature = "cache")]
use std::sync::Arc;

#[cfg(feature = "cache")]
use crate::cache::CacheStore;

#[cfg(feature = "cache")]
use crate::index::hash::compute_content_hash;

/// Executes raw Cypher queries against a [`StorageConnection`].
///
/// Borrows the connection for its lifetime; obtain one via
/// [`CypherExecutor::new`].
pub struct CypherExecutor<'a> {
    conn: &'a StorageConnection,
    #[cfg(feature = "cache")]
    cache: Option<Arc<dyn CacheStore>>,
}

impl<'a> CypherExecutor<'a> {
    /// Creates a new [`CypherExecutor`] borrowing `conn`.
    #[must_use]
    pub fn new(conn: &'a StorageConnection) -> Self {
        Self {
            conn,
            #[cfg(feature = "cache")]
            cache: None,
        }
    }

    /// Creates a new [`CypherExecutor`] with query-result caching enabled.
    ///
    /// Read queries (`MATCH`/`RETURN`/`WITH`) are cached by their SHA-256 key;
    /// write queries bypass the cache and execute directly.
    #[cfg(feature = "cache")]
    #[must_use]
    pub fn new_with_cache(conn: &'a StorageConnection, cache: Arc<dyn CacheStore>) -> Self {
        Self {
            conn,
            cache: Some(cache),
        }
    }

    /// Executes a Cypher query and returns the columns, rows, and duration.
    ///
    /// When a [`CacheStore`] is attached (via
    /// [`new_with_cache`](Self::new_with_cache)), read queries are served from
    /// cache on hit; misses execute the query and store the result. Write
    /// queries (`CREATE`/`MERGE`/`DELETE`/`SET`/etc.) bypass the cache and
    /// invalidate all cached entries before executing, ensuring subsequent
    /// read queries return fresh data.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::InvalidQuery`] if `cypher` is empty or whitespace.
    /// Returns [`QueryError::Storage`] if the underlying database fails.
    pub fn execute(&self, cypher: &str) -> Result<QueryResult> {
        if cypher.trim().is_empty() {
            return Err(QueryError::InvalidQuery(
                "cypher query must not be empty".to_string(),
            ));
        }

        #[cfg(feature = "cache")]
        if let Some(ref cache) = self.cache {
            if Self::is_read_query(cypher) {
                let key = format!("cypher:{}", compute_content_hash(cypher.as_bytes()));
                if let Some(bytes) = cache.get(&key) {
                    // Cache hit: parse UTF-8 result. Fall through on parse
                    // failure (corrupt entry) to recompute and overwrite.
                    if let Ok(result) = serde_json::from_slice::<QueryResult>(&bytes) {
                        return Ok(result);
                    }
                }
                // Cache miss: execute, store, return.
                let result = self.execute_internal(cypher)?;
                // Serialization failure is non-fatal — skip caching and
                // return the result. The next call will recompute.
                if let Ok(bytes) = serde_json::to_vec(&result) {
                    cache.set(&key, bytes);
                }
                return Ok(result);
            }
            // Write query: invalidate cache before executing to ensure
            // subsequent read queries don't return stale results.
            cache.invalidate_all();
        }

        self.execute_internal(cypher)
    }

    /// Executes the query against the storage connection (uncached path).
    fn execute_internal(&self, cypher: &str) -> Result<QueryResult> {
        let start = Instant::now();
        let (columns, rows) = self.conn.query_with_columns(cypher)?;
        let duration_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
        // Single-line for coverage: tarpaulin attribute continuation
        Ok(QueryResult {
            columns,
            rows,
            duration_ms,
        })
    }

    /// Returns `true` if `cypher` is a read query (`MATCH`/`RETURN`/`WITH`).
    ///
    /// A query is considered read-only when:
    /// 1. It begins with a read clause keyword (`MATCH`/`RETURN`/`WITH`), and
    /// 2. It does not contain any write keyword (`CREATE`/`DELETE`/`MERGE`/
    ///    `SET`/`REMOVE`/`DROP`/`FOREACH`/`CALL`) as a standalone word.
    ///
    /// The write-keyword scan is conservative: a query like
    /// `MATCH (n) WHERE n.name = 'DELETE' RETURN n` is treated as a write
    /// query (cache bypassed, performance loss) rather than risk caching a
    /// write operation's result. This trades a small performance hit for
    /// correctness — never cache a write.
    #[cfg(feature = "cache")]
    fn is_read_query(cypher: &str) -> bool {
        let upper = cypher.trim_start().to_uppercase();
        let starts_with_read = upper.starts_with("MATCH")
            || upper.starts_with("RETURN")
            || upper.starts_with("WITH");
        if !starts_with_read {
            return false;
        }
        // Reject queries containing write keywords as whole words.
        // Pad with spaces so leading/trailing keywords are also bounded.
        let padded = format!(" {} ", upper);
        const WRITE_KEYWORDS: &[&str] = &[
            " CREATE ",
            " DELETE ",
            " DETACH ",
            " MERGE ",
            " SET ",
            " REMOVE ",
            " DROP ",
            " FOREACH ",
            " CALL ",
        ];
        !WRITE_KEYWORDS.iter().any(|kw| padded.contains(kw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Language, Node, NodeLabel};
    use crate::storage::Repository;

    /// Builds a fresh in-memory repository with the schema initialized.
    fn fresh_repo() -> Repository {
        Repository::in_memory().expect("in_memory repository")
    }

    /// Builds a sample Function node.
    fn sample_function(id: &str, project: &str, name: &str, qn: &str) -> Node {
        Node::builder(NodeLabel::Function, name, qn)
            .id(id)
            .project(project)
            .file_path("/src/main.rs")
            .start_line(1)
            .end_line(10)
            .language(Language::Rust)
            .signature("fn main()")
            .build()
    }

    #[test]
    fn new_borrows_connection() {
        let repo = fresh_repo();
        let conn = repo.connection();
        let _executor = CypherExecutor::new(conn);
    }

    #[test]
    fn execute_returns_columns_and_rows() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_function("f1", "demo", "alpha", "demo.alpha"),
                sample_function("f2", "demo", "beta", "demo.beta"),
            ],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let executor = CypherExecutor::new(repo.connection());
        let result = executor
            .execute("MATCH (f:Function) RETURN f.name AS name ORDER BY f.name;")
            .expect("execute");

        assert_eq!(result.columns, vec!["name"]);
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0][0], serde_json::json!("alpha"));
        assert_eq!(result.rows[1][0], serde_json::json!("beta"));
    }

    #[test]
    fn execute_ac_query_001_returns_function_names() {
        // AC-QUERY-001: query "MATCH (f:Function) RETURN f.name LIMIT 10"
        // returns function names.
        let repo = fresh_repo();
        let mut nodes = Vec::new();
        for i in 0..15 {
            nodes.push(sample_function(
                &format!("f{i}"),
                "demo",
                &format!("func_{i}"),
                &format!("demo.func_{i}"),
            ));
        }
        repo.save_nodes(&nodes, NodeLabel::Function)
            .expect("save_nodes");

        let executor = CypherExecutor::new(repo.connection());
        let result = executor
            .execute("MATCH (f:Function) RETURN f.name LIMIT 10;")
            .expect("execute");

        assert_eq!(result.columns, vec!["f.name"]);
        assert_eq!(result.rows.len(), 10, "LIMIT 10 should cap results");
        for row in &result.rows {
            assert!(row[0].as_str().unwrap().starts_with("func_"));
        }
    }

    #[test]
    fn execute_empty_result_returns_empty_rows() {
        let repo = fresh_repo();
        let executor = CypherExecutor::new(repo.connection());
        let result = executor
            .execute("MATCH (f:Function) RETURN f.name AS name;")
            .expect("execute");
        assert!(result.rows.is_empty());
        // Columns are still returned even with zero rows.
        assert_eq!(result.columns, vec!["name"]);
    }

    #[test]
    fn execute_invalid_cypher_returns_error() {
        let repo = fresh_repo();
        let executor = CypherExecutor::new(repo.connection());
        let err = executor
            .execute("MATCH (a:Person RETURN a.name;")
            .expect_err("invalid cypher should error");
        assert!(matches!(err, QueryError::Storage(_)));
        assert!(err.to_string().contains("storage error"));
    }

    #[test]
    fn execute_empty_query_returns_invalid_query_error() {
        let repo = fresh_repo();
        let executor = CypherExecutor::new(repo.connection());
        let err = executor.execute("").expect_err("empty query should error");
        assert!(err.is_invalid_query());
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn execute_whitespace_query_returns_invalid_query_error() {
        let repo = fresh_repo();
        let executor = CypherExecutor::new(repo.connection());
        let err = executor
            .execute("   \n\t  ")
            .expect_err("whitespace query should error");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn execute_records_duration() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function("f1", "demo", "main", "demo.main")],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let executor = CypherExecutor::new(repo.connection());
        let result = executor
            .execute("MATCH (f:Function) RETURN f.name AS name;")
            .expect("execute");
        // Duration is recorded; for an in-memory query it should be small but
        // non-negative. We only assert it is finite (not saturating).
        assert!(result.duration_ms < u64::MAX);
    }

    #[test]
    fn execute_create_and_match_roundtrip() {
        let repo = fresh_repo();
        let executor = CypherExecutor::new(repo.connection());
        // Use the executor itself to CREATE data, then MATCH it back.
        executor
            .execute("CREATE (:Project {id: 'p1', name: 'demo', rootPath: '/', language: 'rust', fileCount: 0, indexedAt: 0});")
            .expect("create");
        let result = executor
            .execute("MATCH (p:Project) RETURN p.name AS name, p.id AS id;")
            .expect("match");
        assert_eq!(result.columns, vec!["name", "id"]);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], serde_json::json!("demo"));
        assert_eq!(result.rows[0][1], serde_json::json!("p1"));
    }

    #[test]
    fn execute_multiple_columns_preserve_order() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function("f1", "demo", "main", "demo.main")],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        let executor = CypherExecutor::new(repo.connection());
        let result = executor
            .execute("MATCH (f:Function) RETURN f.name AS name, f.qualifiedName AS qn, f.startLine AS line;")
            .expect("execute");
        assert_eq!(result.columns, vec!["name", "qn", "line"]);
        assert_eq!(result.rows[0][0], serde_json::json!("main"));
        assert_eq!(result.rows[0][1], serde_json::json!("demo.main"));
        assert_eq!(result.rows[0][2], serde_json::json!(1));
    }
}

#[cfg(all(test, feature = "cache"))]
mod cached_tests {
    use super::*;
    use crate::cache::CacheStore;
    use crate::index::hash::compute_content_hash;
    use crate::model::{Language, Node, NodeLabel};
    use crate::storage::Repository;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Mock `CacheStore` for testing — counts get/set/invalidate calls and
    /// stores entries in an in-memory `HashMap`.
    struct CountingCache {
        gets: AtomicUsize,
        sets: AtomicUsize,
        invalidates: AtomicUsize,
        inner: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl CountingCache {
        fn new() -> Self {
            Self {
                gets: AtomicUsize::new(0),
                sets: AtomicUsize::new(0),
                invalidates: AtomicUsize::new(0),
                inner: Mutex::new(HashMap::new()),
            }
        }

        fn gets(&self) -> usize {
            self.gets.load(Ordering::SeqCst)
        }

        fn sets(&self) -> usize {
            self.sets.load(Ordering::SeqCst)
        }

        #[allow(dead_code)]
        fn invalidates(&self) -> usize {
            self.invalidates.load(Ordering::SeqCst)
        }

        /// Returns a snapshot of all cached entries (without counting a get).
        fn snapshot(&self) -> HashMap<String, Vec<u8>> {
            self.inner.lock().expect("lock").clone()
        }
    }

    impl CacheStore for CountingCache {
        fn get(&self, key: &str) -> Option<Vec<u8>> {
            self.gets.fetch_add(1, Ordering::SeqCst);
            self.inner.lock().expect("lock").get(key).cloned()
        }

        fn set(&self, key: &str, val: Vec<u8>) {
            self.sets.fetch_add(1, Ordering::SeqCst);
            self.inner
                .lock()
                .expect("lock")
                .insert(key.to_string(), val);
        }

        fn invalidate_all(&self) {
            self.invalidates.fetch_add(1, Ordering::SeqCst);
            self.inner.lock().expect("lock").clear();
        }
    }

    /// Builds a fresh in-memory repository with the schema initialized.
    fn fresh_repo() -> Repository {
        Repository::in_memory().expect("in_memory repository")
    }

    /// Builds a sample Function node.
    fn sample_function(id: &str, project: &str, name: &str, qn: &str) -> Node {
        Node::builder(NodeLabel::Function, name, qn)
            .id(id)
            .project(project)
            .file_path("/src/main.rs")
            .start_line(1)
            .end_line(10)
            .language(Language::Rust)
            .signature("fn main()")
            .build()
    }

    #[test]
    fn cached_read_query_miss_then_hit() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function("f1", "demo", "alpha", "demo.alpha")],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let cache = Arc::new(CountingCache::new());
        let executor = CypherExecutor::new_with_cache(repo.connection(), cache.clone());
        let cypher = "MATCH (f:Function) RETURN f.name AS name;";

        // First call: cache miss → execute + store.
        let r1 = executor.execute(cypher).expect("first execute");
        assert_eq!(cache.gets(), 1, "first call: cache miss → get");
        assert_eq!(cache.sets(), 1, "first call: store result → set");

        // Second call: cache hit → return cached, no new store.
        let r2 = executor.execute(cypher).expect("second execute");
        assert_eq!(cache.gets(), 2, "second call: cache hit → get");
        assert_eq!(cache.sets(), 1, "second call: no store on hit");

        // Cached result must match the first execution.
        assert_eq!(r1.columns, r2.columns);
        assert_eq!(r1.rows, r2.rows);
    }

    #[test]
    fn cached_read_query_skips_storage_on_hit() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function("f1", "demo", "alpha", "demo.alpha")],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let cache = Arc::new(CountingCache::new());
        let executor = CypherExecutor::new_with_cache(repo.connection(), cache.clone());
        let cypher = "MATCH (f:Function) RETURN f.name AS name;";

        executor.execute(cypher).expect("first (miss)");
        assert_eq!(cache.sets(), 1, "first call should store");

        executor.execute(cypher).expect("second (hit)");
        // sets stays at 1 → second call did NOT go through execute_internal.
        assert_eq!(
            cache.sets(),
            1,
            "hit must not re-execute or re-store"
        );
    }

    #[test]
    fn cached_write_query_not_cached() {
        let repo = fresh_repo();
        let cache = Arc::new(CountingCache::new());
        let executor = CypherExecutor::new_with_cache(repo.connection(), cache.clone());

        // CREATE is a write query — must not touch the cache.
        executor
            .execute("CREATE (:Project {id: 'p1', name: 'demo', rootPath: '/', language: 'rust', fileCount: 0, indexedAt: 0});")
            .expect("create");

        assert_eq!(cache.gets(), 0, "write query must not query cache");
        assert_eq!(cache.sets(), 0, "write query must not store in cache");
    }

    #[test]
    fn cached_query_different_cypher_different_key() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function("f1", "demo", "alpha", "demo.alpha")],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let cache = Arc::new(CountingCache::new());
        let executor = CypherExecutor::new_with_cache(repo.connection(), cache.clone());

        let q1 = "MATCH (f:Function) RETURN f.name AS name;";
        let q2 = "MATCH (f:Function) RETURN f.qualifiedName AS qn;";

        executor.execute(q1).expect("q1");
        executor.execute(q2).expect("q2");

        // Two distinct queries → two cache entries (two gets + two sets).
        assert_eq!(cache.gets(), 2, "two distinct queries → two gets");
        assert_eq!(cache.sets(), 2, "two distinct queries → two sets");
    }

    #[test]
    fn cached_query_with_none_cache_works_normally() {
        // CypherExecutor::new (no cache) behaves exactly as before.
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function("f1", "demo", "alpha", "demo.alpha")],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let executor = CypherExecutor::new(repo.connection());
        let result = executor
            .execute("MATCH (f:Function) RETURN f.name AS name;")
            .expect("execute");
        assert_eq!(result.columns, vec!["name"]);
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn cached_query_stores_correct_result() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function("f1", "demo", "alpha", "demo.alpha")],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let cache = Arc::new(CountingCache::new());
        let executor = CypherExecutor::new_with_cache(repo.connection(), cache.clone());
        let cypher = "MATCH (f:Function) RETURN f.name AS name;";
        let result = executor.execute(cypher).expect("execute");

        // Verify the cached entry deserializes to the same result.
        let snap = cache.snapshot();
        assert_eq!(snap.len(), 1, "exactly one entry should be cached");
        let (key, bytes) = snap.iter().next().expect("one entry");
        assert!(
            key.starts_with("cypher:"),
            "key should have 'cypher:' prefix, got: {key}"
        );
        // Key suffix must be the SHA-256 of the cypher string.
        let expected_hash = compute_content_hash(cypher.as_bytes());
        assert!(
            key.ends_with(&expected_hash),
            "key suffix should be content hash"
        );
        let cached: QueryResult =
            serde_json::from_slice(bytes).expect("deserialize cached result");
        assert_eq!(cached.columns, result.columns);
        assert_eq!(cached.rows, result.rows);
    }
}
