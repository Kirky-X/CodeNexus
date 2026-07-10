// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! trait-kit module for the Trace subsystem (T6/unified-architecture
//! Phase 2, Task 2.10).
//!
//! Implements [`Module`] / [`ModuleBuilder`] / [`WithConfig`] for
//! [`TraceModule`], wiring the existing [`TraceFacade`] (Facade pattern) into
//! the unified Kit registry as `Arc<dyn TraceEngine>` under
//! [`TraceKey`](crate::kit::TraceKey).
//!
//! # Graph ownership
//!
//! [`TraceFacade`] borrows a `&Graph` (lifetime-bound), so it cannot be
//! stored directly inside the capability. The concrete impl
//! ([`TraceCapability`]) instead owns a `db_path: PathBuf` and loads a fresh
//! subgraph per `trace` call via [`load_graph_for_symbol`]. This matches the
//! existing `trace_cmd::run` semantics — every CLI invocation loads a fresh
//! subgraph from the database. A future optimization could cache the graph
//! behind an `RwLock` with explicit invalidation hooks; out of scope for
//! Task 2.10.
//!
//! # Dependency note
//!
//! Conceptually the Trace engine depends on `StorageKey` + `ResolverKey`
//! (it reads the graph that the Indexer/Resolver wrote). The concrete
//! [`TraceCapability`] is self-contained, however: it opens its own
//! [`Repository`](crate::storage::Repository) from the supplied `db_path`
//! and loads the subgraph itself. Therefore `Requirements = NoRequirements`
//! at the type level; the bootstrap (Task 2.13) enforces build ordering
//! (Storage → ... → Resolver → Trace). This mirrors the
//! [`QueryModule`](crate::query::module::QueryModule) design (Task 2.9) —
//! see `design.md` D1 for the rationale.
//!
//! [`Module`]: crate::kit::Module
//! [`ModuleBuilder`]: crate::kit::ModuleBuilder
//! [`WithConfig`]: crate::kit::WithConfig
//! [`load_graph_for_symbol`]: super::graph_loader::load_graph_for_symbol

use std::path::PathBuf;
use std::sync::Arc;

use crate::kit::{Module, ModuleBuilder, NoRequirements, WithConfig};
use crate::storage::StorageError;

use super::capability::TraceEngine;
use super::error::TraceError;
use super::facade::{TraceFacade, TraceType};
use super::graph_loader::load_graph_for_symbol;
use super::TraceResult;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for [`TraceModule`] (Task 2.10).
///
/// Stored in Kit under [`TraceConfigKey`](crate::kit::TraceConfigKey) and
/// injected into [`TraceModuleBuilder`] via [`WithConfig`]. The Trace engine
/// needs only the database path — the capability loads a fresh subgraph per
/// `trace` call via [`load_graph_for_symbol`].
#[derive(Debug, Clone)]
pub struct TraceConfig {
    /// Filesystem path to the LadybugDB database directory.
    ///
    /// Pass `":memory:"` for an in-memory database (useful for tests).
    pub db_path: PathBuf,
}

impl TraceConfig {
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

/// trait-kit module tag for the Trace subsystem (Task 2.10).
///
/// Zero-sized marker — construction logic lives in
/// [`TraceModuleBuilder::build`]. Register in Kit via:
///
/// ```ignore
/// use codenexus::kit::{IntoKitModuleBuilder, Kit, TraceKey};
/// use codenexus::trace::{TraceConfig, TraceModuleBuilder};
///
/// let kit = Kit::new();
/// let trace = TraceModuleBuilder::new()
///     .config(TraceConfig::in_memory())
///     .kit(&kit)
///     .provide::<TraceKey>()?;
/// ```
pub struct TraceModule;

/// Builder for [`TraceModule`] (Task 2.10).
///
/// Construct with [`TraceModuleBuilder::new`], inject config with
/// [`WithConfig::config`], then attach to a [`Kit`](crate::kit::Kit) via
/// [`IntoKitModuleBuilder::kit`](crate::kit::IntoKitModuleBuilder::kit) and
/// call [`provide`](crate::kit::KitModuleBuilder::provide).
pub struct TraceModuleBuilder {
    config: Option<TraceConfig>,
}

impl TraceModuleBuilder {
    /// Creates a builder with no config set. Call `.config(...)` before
    /// building.
    #[must_use]
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for TraceModuleBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for TraceModule {
    type Config = TraceConfig;
    type Requirements = NoRequirements;
    type Capability = Arc<dyn TraceEngine>;
    type Error = TraceError;
    type Builder = TraceModuleBuilder;
    const NAME: &'static str = "trace";
}

impl ModuleBuilder<TraceModule> for TraceModuleBuilder {
    fn build(self) -> Result<Arc<dyn TraceEngine>, TraceError> {
        let config = self.config.ok_or_else(|| {
            TraceError::Storage(StorageError::InvalidData(
                "TraceModuleBuilder requires config — call .config(TraceConfig { db_path }) before build"
                    .to_string(),
            ))
        })?;
        Ok(Arc::new(TraceCapability {
            db_path: config.db_path,
        }))
    }
}

impl WithConfig<TraceModule> for TraceModuleBuilder {
    fn config(self, config: TraceConfig) -> Self {
        Self {
            config: Some(config),
        }
    }
}

// ---------------------------------------------------------------------------
// Concrete dyn TraceEngine implementation
// ---------------------------------------------------------------------------

/// Concrete implementation of [`dyn TraceEngine`] that loads a fresh subgraph
/// from the database on every `trace` call.
///
/// The capability owns only a `db_path` (immutable, `Send + Sync`). Each
/// [`TraceEngine::trace`] invocation:
///
/// 1. Opens a fresh [`Repository`](crate::storage::Repository) via
///    [`load_graph_for_symbol`].
/// 2. Builds a transient [`TraceFacade`] over the loaded subgraph.
/// 3. Delegates to [`TraceFacade::trace`] for symbol resolution + BFS.
///
/// This matches the existing `trace_cmd::run` semantics (one subgraph per
/// CLI invocation). The repository connection is short-lived — opened,
/// queried, and dropped within a single `trace` call.
struct TraceCapability {
    /// Database path used to load subgraphs.
    db_path: PathBuf,
}

impl TraceEngine for TraceCapability {
    fn trace(
        &self,
        symbol: &str,
        trace_type: TraceType,
        depth: usize,
    ) -> std::result::Result<TraceResult, TraceError> {
        // Load the subgraph reachable from `symbol` within `depth` hops.
        // StorageError → TraceError::Storage via the `From` impl in error.rs.
        let graph = load_graph_for_symbol(&self.db_path, symbol, depth)?;
        let facade = TraceFacade::new(&graph);
        facade.trace(symbol, trace_type, depth)
    }

    fn load_graph(
        &self,
        symbol: &str,
        depth: usize,
    ) -> std::result::Result<crate::model::Graph, TraceError> {
        // Delegate to the shared graph loader (Task 2.10 graph_loader.rs).
        // `impact_cmd::run` uses this to obtain the raw Graph for
        // ImpactAnalyzer, which cannot be expressed via `trace()` (that
        // returns a TraceResult, not the graph itself).
        // StorageError → TraceError::Storage via the `From` impl in error.rs.
        Ok(load_graph_for_symbol(&self.db_path, symbol, depth)?)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::TraceKey;
    use crate::storage::StorageConnection;
    use tempfile::TempDir;

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("trace_module_testdb");
        std::mem::forget(dir);
        path
    }

    /// Seeds the database with two functions and a CALLS edge between them.
    fn seed_call_graph(db: &std::path::Path) {
        let conn = StorageConnection::open(db).expect("open");
        conn.init_schema().expect("init_schema");
        conn.execute("CREATE (:Function {id: 'f_a', project: 'demo', name: 'a', qualifiedName: 'demo.a', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create a");
        conn.execute("CREATE (:Function {id: 'f_b', project: 'demo', name: 'b', qualifiedName: 'demo.b', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create b");
        conn.execute("CREATE (:CodeRelation {id: 'e1', source: 'f_a', target: 'f_b', type: 'CALLS', confidence: 1.0, reason: 'direct call', startLine: 2, project: 'demo'});").expect("create edge");
    }

    /// Builds a TraceModule capability backed by an on-disk database seeded
    /// with a simple `a -[CALLS]-> b` call graph.
    fn build_trace_seeded() -> (Arc<dyn TraceEngine>, std::path::PathBuf) {
        let db = fresh_db_path();
        seed_call_graph(&db);
        let cap = TraceModuleBuilder::new()
            .config(TraceConfig {
                db_path: db.clone(),
            })
            .build()
            .expect("TraceModuleBuilder::build");
        (cap, db)
    }

    #[test]
    fn builder_requires_config() {
        let result = TraceModuleBuilder::new().build();
        assert!(result.is_err());
        let err = result.err().unwrap();
        let msg = err.to_string();
        assert!(
            msg.contains("config"),
            "missing-config error should mention config: {msg}"
        );
    }

    #[test]
    fn build_returns_send_sync_capability() {
        let (cap, _db) = build_trace_seeded();
        // If this compiles, TraceCapability is Send + Sync (the dyn
        // TraceEngine bound requires it). The Arc<dyn TraceEngine> is also
        // Send + Sync.
        fn _assert_send_sync<T: Send + Sync>(_: &T) {}
        _assert_send_sync(&cap);
    }

    #[test]
    fn capability_trace_calls_returns_path() {
        let (cap, _db) = build_trace_seeded();
        let result = cap.trace("a", TraceType::Calls, 3).expect("trace");
        assert_eq!(result.symbol, "a");
        assert_eq!(result.paths.len(), 1, "should have 1 call path a->b");
        // The single path should contain both a and b.
        let names: Vec<&str> = result.paths[0]
            .nodes
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert_eq!(names, vec!["a", "b"]);
        assert_eq!(result.paths[0].edges[0].edge_type, "CALLS");
    }

    #[test]
    fn capability_trace_symbol_not_found_returns_error() {
        let (cap, _db) = build_trace_seeded();
        let result = cap.trace("missing", TraceType::Calls, 3);
        assert!(
            matches!(result, Err(TraceError::SymbolNotFound(_))),
            "got {result:?}"
        );
    }

    #[test]
    fn capability_trace_invalid_depth_returns_error() {
        let (cap, _db) = build_trace_seeded();
        let result = cap.trace("a", TraceType::Calls, 0);
        assert!(matches!(result, Err(TraceError::InvalidDepth(0))));
    }

    #[test]
    fn capability_trace_missing_db_returns_storage_error() {
        let cap = TraceModuleBuilder::new()
            .config(TraceConfig {
                db_path: std::path::PathBuf::from("/nonexistent/db.lbug"),
            })
            .build()
            .expect("build");
        let result = cap.trace("a", TraceType::Calls, 3);
        assert!(
            matches!(result, Err(TraceError::Storage(_))),
            "missing db → Storage error, got {result:?}"
        );
    }

    /// Verify the full Kit registration flow works end-to-end.
    #[test]
    fn kit_registration_flow() {
        use crate::kit::{IntoKitModuleBuilder, Kit};

        let db = fresh_db_path();
        seed_call_graph(&db);

        let kit = Kit::new();
        let trace = TraceModuleBuilder::new()
            .config(TraceConfig {
                db_path: db.clone(),
            })
            .kit(&kit)
            .provide::<TraceKey>()
            .expect("provide::<TraceKey>");

        assert!(kit.contains::<TraceKey>());

        let required = kit.require::<TraceKey>().expect("require::<TraceKey>");
        assert!(Arc::ptr_eq(&trace, &required));

        // The registered capability should be callable.
        let result = required.trace("a", TraceType::Calls, 3).expect("trace");
        assert_eq!(result.symbol, "a");
    }

    #[test]
    fn builder_default_equals_new() {
        // Default impl must produce the same state as new() (no config).
        let default_builder = TraceModuleBuilder::default();
        assert!(
            default_builder.build().is_err(),
            "default builder should require config"
        );
    }

    #[test]
    fn trace_config_in_memory_sets_memory_path() {
        let config = TraceConfig::in_memory();
        assert_eq!(config.db_path, std::path::PathBuf::from(":memory:"));
    }
}
