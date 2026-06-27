// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! trait-kit module for the Query subsystem (T6/unified-architecture
//! Phase 2, Task 2.9).
//!
//! Implements [`Module`] / [`ModuleBuilder`] / [`WithConfig`] for
//! [`QueryModule`], wiring the existing [`QueryFacade`] (Facade pattern) into
//! the unified Kit registry as `Arc<dyn QueryEngine>` under
//! [`QueryKey`](crate::kit::QueryKey).
//!
//! # Interior mutability
//!
//! [`QueryFacade`] owns a [`StorageConnection`](crate::storage::StorageConnection)
//! which is intentionally `!Clone` and whose underlying [`lbug::Database`] is
//! not guaranteed to be `Sync`. To satisfy the `Send + Sync` bound on
//! [`dyn QueryEngine`], the concrete impl ([`QueryCapability`]) wraps the
//! facade in a [`Mutex`] — every operation locks, delegates, and unlocks. This
//! mirrors the [`StorageCapability`](crate::storage::module::StorageCapability)
//! design (Task 2.4).
//!
//! # Dependency note
//!
//! Conceptually the Query engine depends on `StorageKey` (it reads the graph
//! that the Indexer/Storage wrote). The concrete [`QueryFacade`] is
//! self-contained, however: it opens its own [`StorageConnection`] from the
//! supplied `db_path` and initializes the schema itself. Therefore
//! `Requirements = NoRequirements` at the type level; the bootstrap
//! (Task 2.13) enforces build ordering (Storage → ... → Query).
//!
//! [`Module`]: crate::kit::Module
//! [`ModuleBuilder`]: crate::kit::ModuleBuilder
//! [`WithConfig`]: crate::kit::WithConfig

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::kit::{Module, ModuleBuilder, NoRequirements, WithConfig};
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
/// Stored in Kit under [`QueryConfigKey`](crate::kit::QueryConfigKey) and
/// injected into [`QueryModuleBuilder`] via [`WithConfig`]. The Query engine
/// needs only the database path — the facade opens its own
/// [`StorageConnection`](crate::storage::StorageConnection) and initializes
/// the schema itself.
#[derive(Debug, Clone)]
pub struct QueryConfig {
    /// Filesystem path to the LadybugDB database directory.
    ///
    /// Pass `":memory:"` for an in-memory database (useful for tests).
    pub db_path: PathBuf,
}

impl QueryConfig {
    /// Creates a config pointing at an in-memory database.
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            db_path: PathBuf::from(":memory:"),
        }
    }
}

// ---------------------------------------------------------------------------
// Module + Builder
// ---------------------------------------------------------------------------

/// trait-kit module tag for the Query subsystem (Task 2.9).
///
/// Zero-sized marker — construction logic lives in
/// [`QueryModuleBuilder::build`]. Register in Kit via:
///
/// ```ignore
/// use codenexus::kit::{IntoKitModuleBuilder, Kit, QueryKey};
/// use codenexus::query::{QueryConfig, QueryModuleBuilder};
///
/// let kit = Kit::new();
/// let query = QueryModuleBuilder::new()
///     .config(QueryConfig::in_memory())
///     .kit(&kit)
///     .provide::<QueryKey>()?;
/// ```
pub struct QueryModule;

/// Builder for [`QueryModule`] (Task 2.9).
///
/// Construct with [`QueryModuleBuilder::new`], inject config with
/// [`WithConfig::config`], then attach to a [`Kit`](crate::kit::Kit) via
/// [`IntoKitModuleBuilder::kit`](crate::kit::IntoKitModuleBuilder::kit) and
/// call [`provide`](crate::kit::KitModuleBuilder::provide).
pub struct QueryModuleBuilder {
    config: Option<QueryConfig>,
}

impl QueryModuleBuilder {
    /// Creates a builder with no config set. Call `.config(...)` before
    /// building.
    #[must_use]
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for QueryModuleBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for QueryModule {
    type Config = QueryConfig;
    type Requirements = NoRequirements;
    type Capability = Arc<dyn QueryEngine>;
    type Error = QueryError;
    type Builder = QueryModuleBuilder;
    const NAME: &'static str = "query";
}

impl ModuleBuilder<QueryModule> for QueryModuleBuilder {
    fn build(self) -> Result<Arc<dyn QueryEngine>, QueryError> {
        let config = self.config.ok_or_else(|| {
            QueryError::Storage(StorageError::InvalidData(
                "QueryModuleBuilder requires config — call .config(QueryConfig { db_path }) before build"
                    .to_string(),
            ))
        })?;
        // QueryFacade::new opens the StorageConnection AND initializes the
        // schema, so the capability is ready for use immediately.
        let facade = QueryFacade::new(&config.db_path)?;
        Ok(Arc::new(QueryCapability {
            inner: Mutex::new(facade),
        }))
    }
}

impl WithConfig<QueryModule> for QueryModuleBuilder {
    fn config(self, config: QueryConfig) -> Self {
        Self {
            config: Some(config),
        }
    }
}

// ---------------------------------------------------------------------------
// Concrete dyn QueryEngine implementation
// ---------------------------------------------------------------------------

/// Concrete implementation of [`dyn QueryEngine`] wrapping a [`QueryFacade`]
/// behind a [`Mutex`].
///
/// The mutex provides the interior mutability needed to satisfy `Send + Sync`
/// regardless of `lbug::Database`'s thread-safety (see
/// [`StorageCapability`](crate::storage::module::StorageCapability) for the
/// same pattern).
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
    ///
    /// Locks the inner [`QueryFacade`], borrows its [`StorageConnection`] via
    /// [`QueryFacade::connection`], and constructs a [`HybridStrategy`] with
    /// the supplied `embed_client`. The strategy internally falls back to
    /// BM25-only on Windows or when the `Embedding` table is missing, so this
    /// method never fails due to vector unavailability — only on genuine
    /// storage errors.
    ///
    /// [`HybridStrategy`]: crate::embed::HybridStrategy
    /// [`StorageConnection`]: crate::storage::StorageConnection
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
            .map_err(|e| QueryError::Storage(StorageError::Query(e.to_string())))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::QueryKey;

    /// Builds a QueryModule capability backed by an in-memory database.
    fn build_query() -> Arc<dyn QueryEngine> {
        QueryModuleBuilder::new()
            .config(QueryConfig::in_memory())
            .build()
            .expect("QueryModuleBuilder::build")
    }

    /// Seeds the capability's database with one project and two functions via
    /// direct Cypher CREATE (avoids depending on the Repository's CSV loader).
    fn seed_fixture(cap: &Arc<dyn QueryEngine>) {
        cap.cypher(
            "CREATE (:Project {id: 'demo', name: 'demo', rootPath: '/', language: 'rust', fileCount: 2, indexedAt: 0});",
        )
        .expect("create project");
        let funcs = [
            ("f1", "demo", "parse_file", "demo.parse_file", "/src/main.rs", 1),
            ("f2", "demo", "parse_line", "demo.parse_line", "/src/main.rs", 10),
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
    fn builder_requires_config() {
        let result = QueryModuleBuilder::new().build();
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(
            err.to_string().contains("config"),
            "missing-config error should mention config: {err}"
        );
    }

    #[test]
    fn build_returns_send_sync_capability() {
        let cap = build_query();
        // If this compiles, QueryCapability is Send + Sync (the dyn QueryEngine
        // bound requires it). The Arc<dyn QueryEngine> is also Send + Sync.
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
        assert!(
            results
                .iter()
                .all(|r| r.file_path.as_deref() == Some("/src/main.rs"))
        );
    }

    #[test]
    fn capability_fulltext_search_returns_results() {
        let cap = build_query();
        seed_fixture(&cap);
        let results = cap
            .fulltext_search("parse", None, 100)
            .expect("fulltext_search");
        assert!(!results.is_empty());
        assert!(
            results
                .iter()
                .all(|r| r.name.to_ascii_lowercase().contains("parse"))
        );
    }

    /// Verify the full Kit registration flow works end-to-end.
    #[test]
    fn kit_registration_flow() {
        use crate::kit::{IntoKitModuleBuilder, Kit};

        let kit = Kit::new();
        let query = QueryModuleBuilder::new()
            .config(QueryConfig::in_memory())
            .kit(&kit)
            .provide::<QueryKey>()
            .expect("provide::<QueryKey>");

        assert!(kit.contains::<QueryKey>());

        let required = kit.require::<QueryKey>().expect("require::<QueryKey>");
        assert!(Arc::ptr_eq(&query, &required));
    }
}
