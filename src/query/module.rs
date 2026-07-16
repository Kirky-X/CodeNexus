// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! trait-kit module for the Query subsystem (T6/unified-architecture
//! Phase 2, Task 2.9; v0.3.3 AsyncKit migration).
//!
//! Implements [`ModuleMeta`] + [`AsyncAutoBuilder`] for [`QueryModule`],
//! wiring the existing [`QueryFacade`] (Facade pattern) into the unified
//! Kit registry as `Arc<dyn QueryEngine>` under
//! [`QueryModule`](crate::kit::QueryModule).

use std::any::TypeId;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::kit::{AsyncAutoBuilder, AsyncKit, ModuleMeta};
use crate::model::NodeLabel;
use crate::storage::StorageError;

use super::capability::QueryEngine;
use super::error::QueryError;
use super::facade::QueryFacade;
use super::{QueryResult, SearchResult};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for [`QueryModule`] (Task 2.9).
///
/// Stored in Kit via `AsyncKit::set_config` and read in
/// [`AsyncAutoBuilder::build`].
#[derive(Debug, Clone)]
pub struct QueryConfig {
    /// Filesystem path to the LadybugDB database directory.
    ///
    /// Pass `":memory:"` for an in-memory database (useful for tests).
    pub db_path: PathBuf,

    /// Open the DB read-only so multiple processes can read concurrently
    /// (DuckDB/LadybugDB shared-read). Query-only CLI commands set this;
    /// skips schema init (the DB is already indexed). Mirrors
    /// [`StorageConfig::read_only`](crate::storage::StorageConfig).
    pub read_only: bool,
}

impl QueryConfig {
    /// Creates a config pointing at an in-memory database.
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            db_path: PathBuf::from(":memory:"),
            read_only: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Module (ModuleMeta + AsyncAutoBuilder)
// ---------------------------------------------------------------------------

/// trait-kit module tag for the Query subsystem (Task 2.9).
///
/// Zero-sized marker — construction logic lives in
/// [`QueryModule::build_cap`]. Register in Kit via:
///
/// ```ignore
/// use codenexus::kit::{AsyncKit, QueryModule};
/// use codenexus::query::QueryConfig;
///
/// let mut kit = AsyncKit::new();
/// kit.set_config(QueryConfig::in_memory());
/// kit.register::<QueryModule>()?;
/// let kit = kit.build().await?;
/// let query = kit.require::<QueryModule>()?;
/// ```
pub struct QueryModule;

impl ModuleMeta for QueryModule {
    const NAME: &'static str = "query";
    fn dependencies() -> &'static [(&'static str, TypeId)] {
        &[]
    }
}

impl AsyncAutoBuilder for QueryModule {
    type Capability = Arc<dyn QueryEngine>;
    type Error = QueryError;

    fn build<'a>(
        kit: &'a AsyncKit,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Capability, Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            let config = kit
                .config::<QueryConfig>()
                .map_err(|e| QueryError::Storage(StorageError::InvalidData(e.to_string())))?;
            Self::build_cap(&config)
        })
    }
}

impl QueryModule {
    /// Constructs a QueryCapability from the given config.
    ///
    /// Shared between [`AsyncAutoBuilder::build`] and tests.
    pub(crate) fn build_cap(config: &QueryConfig) -> Result<Arc<dyn QueryEngine>, QueryError> {
        let facade = if config.read_only {
            QueryFacade::new_read_only(&config.db_path)?
        } else {
            QueryFacade::new(&config.db_path)?
        };
        Ok(Arc::new(QueryCapability {
            inner: Mutex::new(facade),
        }))
    }
}

// ---------------------------------------------------------------------------
// Concrete dyn QueryEngine implementation
// ---------------------------------------------------------------------------

/// Concrete implementation of [`dyn QueryEngine`] wrapping a [`QueryFacade`]
/// behind a [`Mutex`].
struct QueryCapability {
    inner: Mutex<QueryFacade>,
}

impl QueryEngine for QueryCapability {
    fn cypher(&self, query: &str) -> Result<QueryResult, QueryError> {
        self.inner
            .lock()
            .expect("query lock poisoned")
            .cypher(query)
    }

    fn search(
        &self,
        text: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>, QueryError> {
        self.inner
            .lock()
            .expect("query lock poisoned")
            .search(text, project, limit)
    }

    fn search_by_type(
        &self,
        label: NodeLabel,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>, QueryError> {
        self.inner
            .lock()
            .expect("query lock poisoned")
            .search_by_type(label, project, limit)
    }

    fn search_by_file(
        &self,
        file_path: &str,
        project: Option<&str>,
    ) -> Result<Vec<SearchResult>, QueryError> {
        self.inner
            .lock()
            .expect("query lock poisoned")
            .search_by_file(file_path, project)
    }

    fn fulltext_search(
        &self,
        text: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>, QueryError> {
        self.inner
            .lock()
            .expect("query lock poisoned")
            .fulltext_search(text, project, limit)
    }

    /// Hybrid BM25 + semantic search (Task 2.14 / AC-SEARCH-002).
    #[cfg(feature = "embed")]
    fn semantic_search(
        &self,
        text: &str,
        project: Option<&str>,
        limit: usize,
        embed_client: &dyn crate::embed::EmbedClient,
    ) -> Result<Vec<SearchResult>, QueryError> {
        use crate::embed::{HybridStrategy, SearchStrategy};

        let facade = self.inner.lock().expect("query lock poisoned");
        let strategy = HybridStrategy::new(facade.connection(), embed_client);
        strategy
            .search(text, project, limit)
            .map_err(|e: crate::embed::EmbedError| {
                QueryError::Storage(StorageError::Query(e.to_string()))
            })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{AsyncKit, QueryModule};

    /// Builds a QueryModule capability backed by an in-memory database.
    fn build_query() -> Arc<dyn QueryEngine> {
        QueryModule::build_cap(&QueryConfig::in_memory()).expect("QueryModule::build_cap")
    }

    /// Seeds the capability's database with one project and two functions via
    /// direct Cypher CREATE.
    fn seed_fixture(cap: &Arc<dyn QueryEngine>) {
        cap.cypher(
            "CREATE (:Project {id: 'demo', name: 'demo', rootPath: '/', language: 'rust', fileCount: 2, indexedAt: 0});",
        )
        .expect("create project");
        let funcs = [
            (
                "f1",
                "demo",
                "parse_file",
                "demo.parse_file",
                "/src/main.rs",
                1,
            ),
            (
                "f2",
                "demo",
                "parse_line",
                "demo.parse_line",
                "/src/main.rs",
                10,
            ),
        ];
        for (id, project, name, qn, file, line) in funcs {
            let cypher = format!(
                "CREATE (:Function {{id: '{id}', project: '{project}', name: '{name}', \
                 qualifiedName: '{qn}', filePath: '{file}', startLine: {line}, endLine: {end}, \
                 signature: '', returnType: '', isExported: false, docstring: '', \
                 content: '', parentQn: ''}});",
                end = line + 10,
            );
            cap.cypher(&cypher).expect("create function");
        }
    }

    #[test]
    fn build_returns_send_sync_capability() {
        let cap = build_query();
        fn _assert_send_sync<T: Send + Sync>(_: &T) {}
        _assert_send_sync(&cap);
    }

    #[test]
    fn capability_cypher_executes_query() {
        let cap = build_query();
        seed_fixture(&cap);
        let result = cap
            .cypher("MATCH (f:Function) RETURN f.name AS name ORDER BY f.name;")
            .expect("cypher");
        assert_eq!(result.columns, vec!["name"]);
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0][0], serde_json::json!("parse_file"));
    }

    #[test]
    fn capability_cypher_empty_returns_invalid_query() {
        let cap = build_query();
        let result = cap.cypher("");
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(
            err.is_invalid_query(),
            "empty query → InvalidQuery, got {err:?}"
        );
    }

    #[test]
    fn capability_search_finds_by_name() {
        let cap = build_query();
        seed_fixture(&cap);
        let results = cap.search("parse", None, 100).expect("search");
        let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"parse_file"));
        assert!(names.contains(&"parse_line"));
    }

    #[test]
    fn capability_search_by_type_returns_functions() {
        let cap = build_query();
        seed_fixture(&cap);
        let results = cap
            .search_by_type(NodeLabel::Function, None, 100)
            .expect("search_by_type");
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.label == "Function"));
    }

    #[test]
    fn capability_search_by_file_returns_symbols() {
        let cap = build_query();
        seed_fixture(&cap);
        let results = cap
            .search_by_file("/src/main.rs", None)
            .expect("search_by_file");
        assert_eq!(results.len(), 2);
        assert!(results
            .iter()
            .all(|r| r.file_path.as_deref() == Some("/src/main.rs")));
    }

    #[test]
    fn capability_fulltext_search_returns_results() {
        let cap = build_query();
        seed_fixture(&cap);
        let results = cap
            .fulltext_search("parse", None, 100)
            .expect("fulltext_search");
        assert!(!results.is_empty());
        assert!(results
            .iter()
            .all(|r| r.name.to_ascii_lowercase().contains("parse")));
    }

    /// Verify the full AsyncKit registration flow works end-to-end.
    #[tokio::test]
    async fn kit_registration_flow() {
        let mut kit = AsyncKit::new();
        kit.set_config(QueryConfig::in_memory());
        kit.register::<QueryModule>()
            .expect("register::<QueryModule>");
        let kit = kit.build().await.expect("build");

        assert!(kit.contains::<QueryModule>());

        let required = kit
            .require::<QueryModule>()
            .expect("require::<QueryModule>");
        let result = required
            .cypher("MATCH (f:Function) RETURN f.name AS name;")
            .expect("cypher");
        assert_eq!(result.rows.len(), 0, "empty db → 0 functions");
    }
}
