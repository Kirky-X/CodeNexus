// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Query facade (Facade pattern, PRD §4.4).
//!
//! [`QueryFacade`] is the single entry point for the CLI `query` and `search`
//! commands. It owns a [`StorageConnection`] and delegates to the
//! [`CypherExecutor`], [`StructuredSearcher`], and [`FullTextSearcher`]
//! sub-components.

use std::path::Path;
use std::time::Instant;
use tracing::debug;

use super::cypher::CypherExecutor;
use super::error::Result;
use super::fulltext::FullTextSearcher;
use super::structured::StructuredSearcher;
use super::{QueryResult, SearchResult};
use crate::model::NodeLabel;
use crate::storage::StorageConnection;

/// Facade unifying Cypher execution, structured search, and full-text search.
///
/// Owns the underlying [`StorageConnection`] and orchestrates the sub-searchers.
/// Obtain one via [`QueryFacade::new`] (opens or creates the database).
pub struct QueryFacade {
    conn: StorageConnection,
}

impl QueryFacade {
    /// Opens (or creates) the database at `db_path` and initializes the schema.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::Storage`] if the database cannot be opened or the
    /// schema cannot be initialized.
    pub fn new(db_path: &Path) -> Result<Self> {
        let conn = StorageConnection::open(db_path)?;
        conn.init_schema()?;
        Ok(Self { conn })
    }

    /// Creates a facade over an already-open [`StorageConnection`].
    ///
    /// The caller is responsible for ensuring the schema is initialized.
    #[must_use]
    pub fn with_connection(conn: StorageConnection) -> Self {
        Self { conn }
    }

    /// Returns a reference to the underlying connection (for advanced callers).
    pub fn connection(&self) -> &StorageConnection {
        &self.conn
    }

    /// Executes a raw Cypher query.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::InvalidQuery`] for empty input, or
    /// [`QueryError::Storage`] on database failure.
    pub fn cypher(&self, query: &str) -> Result<QueryResult> {
        let start = Instant::now();
        let result = CypherExecutor::new(&self.conn).execute(query);
        let duration_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
        debug!(
            event = "query_executed",
            query_type = "cypher",
            duration_ms = duration_ms,
            results = result.as_ref().map(|r| r.rows.len()).unwrap_or(0),
            "query executed"
        );
        result
    }

    /// General structured search by name (CONTAINS), sorted by relevance.
    pub fn search(&self, text: &str, project: Option<&str>, limit: usize) -> Result<Vec<SearchResult>> {
        let start = Instant::now();
        let result = StructuredSearcher::new(&self.conn).search(text, project, limit);
        let duration_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
        debug!(
            event = "query_executed",
            query_type = "search_name",
            duration_ms = duration_ms,
            results = result.as_ref().map(|r| r.len()).unwrap_or(0),
            "query executed"
        );
        result
    }

    /// Returns all nodes of the given `label`, optionally filtered by project.
    pub fn search_by_type(
        &self,
        label: NodeLabel,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let start = Instant::now();
        let result = StructuredSearcher::new(&self.conn).search_by_type(label, project, limit);
        let duration_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
        debug!(
            event = "query_executed",
            query_type = "search_type",
            duration_ms = duration_ms,
            results = result.as_ref().map(|r| r.len()).unwrap_or(0),
            "query executed"
        );
        result
    }

    /// Returns all symbols located in `file_path`, optionally filtered by project.
    pub fn search_by_file(
        &self,
        file_path: &str,
        project: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        let start = Instant::now();
        let result = StructuredSearcher::new(&self.conn).search_by_file(file_path, project);
        let duration_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
        debug!(
            event = "query_executed",
            query_type = "search_file",
            duration_ms = duration_ms,
            results = result.as_ref().map(|r| r.len()).unwrap_or(0),
            "query executed"
        );
        result
    }

    /// BM25 full-text search (FTS extension when available, CONTAINS fallback).
    pub fn fulltext_search(
        &self,
        text: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let start = Instant::now();
        let result = FullTextSearcher::new(&self.conn).search(text, project, limit);
        let duration_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
        debug!(
            event = "query_executed",
            query_type = "fulltext",
            duration_ms = duration_ms,
            results = result.as_ref().map(|r| r.len()).unwrap_or(0),
            "query executed"
        );
        result
    }
}

impl std::fmt::Debug for QueryFacade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryFacade")
            .field("conn", &"Opaque StorageConnection")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NodeLabel;
    use crate::query::error::QueryError;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    /// Builds a facade backed by an in-memory database with the schema
    /// initialized.
    fn fresh_facade() -> QueryFacade {
        let conn = StorageConnection::in_memory().expect("in_memory connection");
        conn.init_schema().expect("init_schema");
        QueryFacade::with_connection(conn)
    }

    /// Inserts a fixture dataset into the facade's database via direct Cypher.
    fn seed_fixture(facade: &QueryFacade) {
        // Use the facade's own Cypher executor to insert data.
        facade
            .cypher("CREATE (:Project {id: 'demo', name: 'demo', rootPath: '/', language: 'rust', fileCount: 2, indexedAt: 0});")
            .expect("create project");
        // Insert functions via direct Cypher CREATE (avoids needing the
        // Repository's CSV loader in the facade tests).
        let funcs = [
            ("f1", "demo", "parse_file", "demo.parse_file", "/src/main.rs", 1),
            ("f2", "demo", "parse_line", "demo.parse_line", "/src/main.rs", 10),
            ("f3", "demo", "read_input", "demo.read_input", "/src/lib.rs", 1),
        ];
        for (id, project, name, qn, file, line) in funcs {
            let cypher = format!(
                "CREATE (:Function {{id: '{id}', project: '{project}', name: '{name}', \
                 qualifiedName: '{qn}', filePath: '{file}', startLine: {line}, endLine: {end}, \
                 signature: '', returnType: '', isExported: false, docstring: '', \
                 content: '', parentQn: ''}});",
                end = line + 10,
            );
            facade.cypher(&cypher).expect("create function");
        }
        // Insert a class.
        let cypher = "CREATE (:Class {id: 'c1', project: 'demo', name: 'Parser', \
                      qualifiedName: 'demo.Parser', filePath: '/src/main.rs', \
                      startLine: 20, endLine: 40, isExported: true, docstring: '', \
                      content: '', parentQn: ''});";
        facade.cypher(cypher).expect("create class");
    }

    #[test]
    fn facade_cypher_executes_query() {
        let facade = fresh_facade();
        seed_fixture(&facade);
        let result = facade
            .cypher("MATCH (f:Function) RETURN f.name AS name ORDER BY f.name;")
            .expect("cypher");
        assert_eq!(result.columns, vec!["name"]);
        assert_eq!(result.rows.len(), 3);
        assert_eq!(result.rows[0][0], serde_json::json!("parse_file"));
    }

    #[test]
    fn facade_cypher_ac_query_001() {
        // AC-QUERY-001 via the facade.
        let facade = fresh_facade();
        seed_fixture(&facade);
        let result = facade
            .cypher("MATCH (f:Function) RETURN f.name LIMIT 10;")
            .expect("cypher");
        assert_eq!(result.rows.len(), 3);
    }

    #[test]
    fn facade_search_finds_by_name() {
        let facade = fresh_facade();
        seed_fixture(&facade);
        let results = facade
            .search("parse", None, 100)
            .expect("search");
        let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"parse_file"));
        assert!(names.contains(&"parse_line"));
        assert!(names.contains(&"Parser")); // case-insensitive substring
    }

    #[test]
    fn facade_search_ac_search_001() {
        // AC-SEARCH-001 via the facade.
        let facade = fresh_facade();
        seed_fixture(&facade);
        let results = facade
            .search("parse", None, 100)
            .expect("search");
        assert!(results.iter().any(|r| r.name.contains("parse")));
    }

    #[test]
    fn facade_search_by_type_returns_functions() {
        let facade = fresh_facade();
        seed_fixture(&facade);
        let results = facade
            .search_by_type(NodeLabel::Function, None, 100)
            .expect("search_by_type");
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.label == "Function"));
    }

    #[test]
    fn facade_search_by_type_returns_classes() {
        let facade = fresh_facade();
        seed_fixture(&facade);
        let results = facade
            .search_by_type(NodeLabel::Class, None, 100)
            .expect("search_by_type");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Parser");
    }

    #[test]
    fn facade_search_by_file_returns_symbols_in_file() {
        let facade = fresh_facade();
        seed_fixture(&facade);
        let results = facade
            .search_by_file("/src/main.rs", None)
            .expect("search_by_file");
        // parse_file, parse_line, Parser are all in /src/main.rs.
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.file_path.as_deref() == Some("/src/main.rs")));
    }

    #[test]
    fn facade_fulltext_search_returns_results() {
        let facade = fresh_facade();
        seed_fixture(&facade);
        let results = facade
            .fulltext_search("parse", None, 100)
            .expect("fulltext_search");
        assert!(!results.is_empty());
        assert!(results.iter().all(|r| r.name.to_ascii_lowercase().contains("parse")));
    }

    #[test]
    fn facade_fulltext_search_respects_limit() {
        let facade = fresh_facade();
        seed_fixture(&facade);
        let results = facade
            .fulltext_search("parse", None, 1)
            .expect("fulltext_search");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn facade_search_filters_by_project() {
        let facade = fresh_facade();
        seed_fixture(&facade);
        // Insert a function in a different project.
        facade
            .cypher("CREATE (:Function {id: 'f9', project: 'other', name: 'parse_other', qualifiedName: 'other.parse_other', filePath: '/x.rs', startLine: 1, endLine: 2, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});")
            .expect("create other function");
        let results = facade
            .search("parse", Some("demo"), 100)
            .expect("search");
        assert!(results.iter().all(|r| r.qualified_name.as_ref().unwrap().starts_with("demo.")));
    }

    #[test]
    fn facade_cypher_invalid_returns_error() {
        let facade = fresh_facade();
        let err = facade.cypher("MATCH (a RETURN a;").expect_err("invalid cypher");
        assert!(matches!(err, QueryError::Storage(_)));
    }

    #[test]
    fn facade_cypher_empty_returns_invalid_query() {
        let facade = fresh_facade();
        let err = facade.cypher("").expect_err("empty query");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn facade_new_opens_database_on_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("testdb");
        let facade = QueryFacade::new(&path).expect("QueryFacade::new");
        // Leak the tempdir so LadybugDB's open file handles remain valid for
        // the test's lifetime (mirrors the storage tests' approach).
        std::mem::forget(dir);
        // Schema is initialized: querying an empty Project table works.
        let result = facade
            .cypher("MATCH (p:Project) RETURN count(p) AS cnt;")
            .expect("cypher");
        assert_eq!(result.rows[0][0], serde_json::json!(0));
    }

    #[test]
    fn facade_connection_returns_reference() {
        let facade = fresh_facade();
        let _ = facade.connection();
    }

    #[test]
    fn facade_debug_does_not_panic() {
        let facade = fresh_facade();
        let s = format!("{facade:?}");
        assert!(s.contains("QueryFacade"));
    }

    // --- LOG-003: query_executed event emission ---

    /// A `MakeWriter` that buffers emitted events into a shared `Vec<u8>` so a
    /// test can assert on what the subscriber actually wrote.
    struct CapturingMakeWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl MakeWriter for CapturingMakeWriter {
        type Writer = CapturingWriter;

        fn make_writer(&self) -> Self::Writer {
            CapturingWriter {
                buf: self.buf.clone(),
            }
        }
    }

    struct CapturingWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for CapturingWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.buf.lock().unwrap().write_all(bytes)?;
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Runs `f` inside a scoped tracing subscriber (with DEBUG level) that
    /// captures all event output into a string, returning that string.
    fn capture_tracing_debug<R>(f: impl FnOnce() -> R) -> String {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::FmtSubscriber::builder()
            .with_target(false)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(CapturingMakeWriter { buf: buf.clone() })
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let bytes = buf.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn log_003_cypher_emits_query_executed() {
        let facade = fresh_facade();
        seed_fixture(&facade);

        let captured = capture_tracing_debug(|| {
            let _ = facade
                .cypher("MATCH (f:Function) RETURN f.name AS name LIMIT 10;")
                .expect("cypher");
        });

        assert!(
            captured.contains("query_executed"),
            "LOG-003: query_executed event should be emitted, got: {captured:?}"
        );
        assert!(
            captured.contains("query_type") && captured.contains("cypher"),
            "query_executed should carry query_type=cypher"
        );
        assert!(
            captured.contains("duration_ms"),
            "query_executed should carry duration_ms field"
        );
    }

    #[test]
    fn log_003_search_emits_query_executed() {
        let facade = fresh_facade();
        seed_fixture(&facade);

        let captured = capture_tracing_debug(|| {
            let _ = facade.search("parse", None, 100).expect("search");
        });

        assert!(
            captured.contains("query_executed"),
            "LOG-003: search should emit query_executed, got: {captured:?}"
        );
        assert!(
            captured.contains("search_name"),
            "search should carry query_type=search_name"
        );
    }

    #[test]
    fn log_003_fulltext_emits_query_executed() {
        let facade = fresh_facade();
        seed_fixture(&facade);

        let captured = capture_tracing_debug(|| {
            let _ = facade.fulltext_search("parse", None, 100).expect("fulltext");
        });

        assert!(
            captured.contains("query_executed"),
            "LOG-003: fulltext_search should emit query_executed, got: {captured:?}"
        );
        assert!(
            captured.contains("fulltext"),
            "fulltext_search should carry query_type=fulltext"
        );
    }
}
