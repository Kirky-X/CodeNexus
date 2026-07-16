// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! trait-kit module for the Trace subsystem (T6/unified-architecture
//! Phase 2, Task 2.10; v0.3.3 AsyncKit migration).
//!
//! Implements [`ModuleMeta`] + [`AsyncAutoBuilder`] for [`TraceModule`],
//! wiring the existing [`TraceFacade`] (Facade pattern) into the unified Kit
//! registry as `Arc<dyn TraceEngine>` under
//! [`TraceModule`](crate::kit::TraceModule).
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
//! Conceptually the Trace engine depends on `StorageModule` +
//! `ResolverModule` (it reads the graph that the Indexer/Resolver wrote).
//! The concrete [`TraceCapability`] is self-contained, however: it opens its
//! own [`Repository`](crate::storage::Repository) from the supplied
//! `db_path` and loads the subgraph itself. Therefore `dependencies = &[]`
//! at the type level; the bootstrap (Task 2.13) enforces build ordering
//! (Storage → ... → Resolver → Trace). This mirrors the
//! [`QueryModule`](crate::query::module::QueryModule) design (Task 2.9) —
//! see `design.md` D1 for the rationale.
//!
//! [`load_graph_for_symbol`]: super::graph_loader::load_graph_for_symbol

use std::any::TypeId;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use crate::kit::{AsyncAutoBuilder, AsyncKit, ModuleMeta};
use crate::model::EdgeType;
use crate::storage::StorageError;

use super::capability::TraceEngine;
use super::error::TraceError;
use super::facade::{PathFilter, TraceFacade, TraceType};
use super::graph_loader::{load_graph_for_symbol, MAX_SUBGRAPH_NODES};
use super::TraceResult;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for [`TraceModule`] (Task 2.10) and [`TraceEngine`](super::facade::TraceEngine) (T032).
///
/// Stored in Kit via `AsyncKit::set_config` and read in
/// [`AsyncAutoBuilder::build`]. The Trace engine needs only the database
/// path — the capability loads a fresh subgraph per `trace` call via
/// [`load_graph_for_symbol`].
///
/// T032 extends this config with advanced tracing options: `max_depth`,
/// `edge_types`, `path_filter`, `detect_cycles`, `cross_service`. These
/// fields are consumed by the advanced [`TraceEngine`] struct and the
/// service layer; the kit module ignores them (uses only `db_path`).
///
/// [`TraceEngine`]: super::facade::TraceEngine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceConfig {
    /// Filesystem path to the LadybugDB database directory.
    ///
    /// Pass `":memory:"` for an in-memory database (useful for tests).
    pub db_path: PathBuf,
    /// Maximum trace depth (default 5, max 10 per spec constraint).
    pub max_depth: u32,
    /// Edge types to traverse during tracing (default `[Calls]`).
    pub edge_types: Vec<EdgeType>,
    /// Optional path filter (R-trace-001).
    pub path_filter: Option<PathFilter>,
    /// Whether to detect cycles during tracing (default `false`).
    pub detect_cycles: bool,
    /// Whether to traverse `HttpCalls` edges for cross-service tracing
    /// (default `false`).
    pub cross_service: bool,
    /// Open the DB read-only so multiple processes can read concurrently
    /// (DuckDB/LadybugDB shared-read). Trace-only CLI commands set this;
    /// skips schema init (the DB is already indexed). Mirrors
    /// [`StorageConfig::read_only`](crate::storage::StorageConfig).
    pub read_only: bool,
}

/// Maximum allowed depth (spec constraint: max_depth ≤ 10).
const MAX_DEPTH_LIMIT: u32 = 10;

impl Default for TraceConfig {
    fn default() -> Self {
        Self {
            db_path: PathBuf::from(":memory:"),
            max_depth: 5,
            edge_types: vec![EdgeType::Calls],
            path_filter: None,
            detect_cycles: false,
            cross_service: false,
            read_only: false,
        }
    }
}

impl TraceConfig {
    /// Creates a config pointing at an in-memory database with default
    /// advanced tracing options.
    #[must_use]
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Clamps `max_depth` to the spec-mandated limit of 10.
    #[must_use]
    pub fn clamped_depth(&self) -> u32 {
        self.max_depth.min(MAX_DEPTH_LIMIT)
    }
}

// ---------------------------------------------------------------------------
// Module (ModuleMeta + AsyncAutoBuilder)
// ---------------------------------------------------------------------------

/// trait-kit module tag for the Trace subsystem (Task 2.10).
///
/// Zero-sized marker — construction logic lives in
/// [`TraceModule::build_cap`] (called from the [`AsyncAutoBuilder`] impl).
/// Register the capability in Kit via:
///
/// ```ignore
/// use codenexus::kit::{AsyncKit, TraceModule};
/// use codenexus::trace::TraceConfig;
///
/// let mut kit = AsyncKit::new();
/// kit.set_config(TraceConfig::in_memory());
/// kit.register::<TraceModule>()?;
/// let kit = kit.build().await?;
/// let trace = kit.require::<TraceModule>()?;
/// ```
pub struct TraceModule;

impl ModuleMeta for TraceModule {
    const NAME: &'static str = "trace";
    fn dependencies() -> &'static [(&'static str, TypeId)] {
        &[]
    }
}

impl AsyncAutoBuilder for TraceModule {
    type Capability = Arc<dyn TraceEngine>;
    type Error = TraceError;

    fn build<'a>(
        kit: &'a AsyncKit,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Capability, Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            let config = kit
                .config::<TraceConfig>()
                .map_err(|e| TraceError::Storage(StorageError::InvalidData(e.to_string())))?;
            Self::build_cap(&config)
        })
    }
}

impl TraceModule {
    /// Constructs a TraceCapability from the given config.
    ///
    /// Shared between [`AsyncAutoBuilder::build`] and tests so that
    /// capability-level tests can run without an async runtime.
    pub(crate) fn build_cap(config: &TraceConfig) -> Result<Arc<dyn TraceEngine>, TraceError> {
        Ok(Arc::new(TraceCapability {
            db_path: config.db_path.clone(),
            read_only: config.read_only,
        }))
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
    /// Whether subgraph loads open the DB read-only (concurrent-safe reads).
    read_only: bool,
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
        let (graph, _truncated) = load_graph_for_symbol(
            &self.db_path,
            symbol,
            depth,
            MAX_SUBGRAPH_NODES,
            self.read_only,
        )?;
        let facade = TraceFacade::new(&graph);
        facade.trace(symbol, trace_type, depth)
    }

    fn load_graph(
        &self,
        symbol: &str,
        depth: usize,
        max_nodes: usize,
    ) -> std::result::Result<(crate::model::Graph, bool), TraceError> {
        // Delegate to the shared graph loader (Task 2.10 graph_loader.rs).
        // `impact_cmd::run` uses this to obtain the raw Graph for
        // ImpactAnalyzer, which cannot be expressed via `trace()` (that
        // returns a TraceResult, not the graph itself). The returned `bool` is
        // the `max_nodes` truncation flag surfaced to the impact output.
        // StorageError → TraceError::Storage via the `From` impl in error.rs.
        Ok(load_graph_for_symbol(
            &self.db_path,
            symbol,
            depth,
            max_nodes,
            self.read_only,
        )?)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{AsyncKit, TraceModule};
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
        let cap = TraceModule::build_cap(&TraceConfig {
            db_path: db.clone(),
            ..Default::default()
        })
        .expect("TraceModule::build_cap");
        (cap, db)
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
        let cap = TraceModule::build_cap(&TraceConfig {
            db_path: std::path::PathBuf::from("/nonexistent/db.lbug"),
            ..Default::default()
        })
        .expect("build_cap");
        let result = cap.trace("a", TraceType::Calls, 3);
        assert!(
            matches!(result, Err(TraceError::Storage(_))),
            "missing db → Storage error, got {result:?}"
        );
    }

    /// Verify the full AsyncKit registration flow works end-to-end.
    #[tokio::test]
    async fn kit_registration_flow() {
        let db = fresh_db_path();
        seed_call_graph(&db);

        let mut kit = AsyncKit::new();
        kit.set_config(TraceConfig {
            db_path: db.clone(),
            ..Default::default()
        });
        kit.register::<TraceModule>()
            .expect("register::<TraceModule>");
        let kit = kit.build().await.expect("build");

        assert!(kit.contains::<TraceModule>(), "TraceModule missing");

        let required = kit
            .require::<TraceModule>()
            .expect("require::<TraceModule>");

        // The registered capability should be callable.
        let result = required.trace("a", TraceType::Calls, 3).expect("trace");
        assert_eq!(result.symbol, "a");
    }

    #[test]
    fn trace_config_in_memory_sets_memory_path() {
        let config = TraceConfig::in_memory();
        assert_eq!(config.db_path, std::path::PathBuf::from(":memory:"));
    }
}
