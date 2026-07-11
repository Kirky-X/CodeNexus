// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! trait-kit module for the Storage subsystem (T6/unified-architecture
//! Phase 2, Task 2.4; v0.3.3 AsyncKit migration).
//!
//! Implements [`ModuleMeta`] + [`AsyncAutoBuilder`] for [`StorageModule`],
//! wiring the existing [`Repository`] (which owns a [`StorageConnection`])
//! into the unified Kit registry as `Arc<dyn Storage>` under
//! [`StorageModule`](crate::kit::StorageModule).
//!
//! # Interior mutability
//!
//! [`Repository`] owns a [`StorageConnection`] which is intentionally
//! `!Clone` and whose underlying [`lbug::Database`] is not guaranteed to be
//! `Sync`. To satisfy the `Send + Sync` bound on [`dyn Storage`], the
//! concrete impl ([`StorageCapability`]) wraps the repository in a
//! [`Mutex`] — every operation locks, delegates, and unlocks. This is the
//! design documented in [`capability.rs`](super::capability).

use std::any::TypeId;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::kit::{AsyncAutoBuilder, AsyncKit, ModuleMeta};

use super::capability::Storage;
use super::connection::SchemaInitReport;
use super::error::StorageError;
use super::repository::{FunctionRecord, ProjectRecord, Repository};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for [`StorageModule`] (Task 2.4).
///
/// Stored in Kit via `AsyncKit::set_config` and read in
/// [`AsyncAutoBuilder::build`].
#[derive(Debug, Clone)]
pub struct StorageConfig {
    /// Filesystem path to the LadybugDB database directory.
    ///
    /// Pass `":memory:"` for an in-memory database (useful for tests).
    pub db_path: PathBuf,
}

impl StorageConfig {
    /// Creates a config pointing at an in-memory database.
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            db_path: PathBuf::from(":memory:"),
        }
    }
}

// ---------------------------------------------------------------------------
// Module (ModuleMeta + AsyncAutoBuilder)
// ---------------------------------------------------------------------------

/// trait-kit module tag for the Storage subsystem (Task 2.4).
///
/// Zero-sized marker — construction logic lives in
/// [`StorageModule::build_cap`] (called from the [`AsyncAutoBuilder`]
/// impl). Register the capability in Kit via:
///
/// ```ignore
/// use codenexus::kit::{AsyncKit, StorageModule};
/// use codenexus::storage::StorageConfig;
///
/// let mut kit = AsyncKit::new();
/// kit.set_config(StorageConfig { db_path: ":memory:".into() });
/// kit.register::<StorageModule>()?;
/// let kit = kit.build().await?;
/// let storage = kit.require::<StorageModule>()?;
/// ```
pub struct StorageModule;

impl ModuleMeta for StorageModule {
    const NAME: &'static str = "storage";
    fn dependencies() -> &'static [(&'static str, TypeId)] {
        &[]
    }
}

impl AsyncAutoBuilder for StorageModule {
    type Capability = Arc<dyn Storage>;
    type Error = StorageError;

    fn build<'a>(
        kit: &'a AsyncKit,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Capability, Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            let config = kit
                .config::<StorageConfig>()
                .map_err(|e| StorageError::InvalidData(e.to_string()))?;
            Self::build_cap(&config)
        })
    }
}

impl StorageModule {
    /// Constructs a StorageCapability from the given config.
    ///
    /// Shared between [`AsyncAutoBuilder::build`] and tests so that
    /// capability-level tests can run without an async runtime.
    pub(crate) fn build_cap(config: &StorageConfig) -> Result<Arc<dyn Storage>, StorageError> {
        let repo = Repository::open(&config.db_path)?;
        Ok(Arc::new(StorageCapability {
            inner: Mutex::new(repo),
        }))
    }
}

// ---------------------------------------------------------------------------
// Concrete dyn Storage implementation
// ---------------------------------------------------------------------------

/// Concrete implementation of [`dyn Storage`] wrapping a [`Repository`] behind
/// a [`Mutex`].
///
/// The mutex provides the interior mutability needed to satisfy `Send + Sync`
/// regardless of `lbug::Database`'s thread-safety (see
/// [`capability.rs`](super::capability) design note).
struct StorageCapability {
    inner: Mutex<Repository>,
}

impl Storage for StorageCapability {
    fn init_schema(&self) -> Result<SchemaInitReport, StorageError> {
        self.inner
            .lock()
            .expect("storage lock poisoned")
            .init_schema()
    }

    fn execute(&self, cypher: &str) -> Result<(), StorageError> {
        self.inner
            .lock()
            .expect("storage lock poisoned")
            .connection()
            .execute(cypher)
    }

    fn query(&self, cypher: &str) -> Result<Vec<Vec<serde_json::Value>>, StorageError> {
        self.inner
            .lock()
            .expect("storage lock poisoned")
            .connection()
            .query(cypher)
    }

    fn save_project(&self, node: &crate::model::Node) -> Result<(), StorageError> {
        self.inner
            .lock()
            .expect("storage lock poisoned")
            .save_project(node)
    }

    fn save_nodes(
        &self,
        nodes: &[crate::model::Node],
        label: crate::model::NodeLabel,
    ) -> Result<(), StorageError> {
        self.inner
            .lock()
            .expect("storage lock poisoned")
            .save_nodes(nodes, label)
    }

    fn save_edges(&self, edges: &[crate::model::Edge]) -> Result<(), StorageError> {
        self.inner
            .lock()
            .expect("storage lock poisoned")
            .save_edges(edges)
    }

    fn get_project(&self, id: &str) -> Result<Option<ProjectRecord>, StorageError> {
        self.inner
            .lock()
            .expect("storage lock poisoned")
            .get_project(id)
    }

    fn list_projects(&self) -> Result<Vec<ProjectRecord>, StorageError> {
        self.inner
            .lock()
            .expect("storage lock poisoned")
            .list_projects()
    }

    fn query_functions(&self, project: &str) -> Result<Vec<FunctionRecord>, StorageError> {
        self.inner
            .lock()
            .expect("storage lock poisoned")
            .query_functions(project)
    }

    fn get_file_hash(
        &self,
        file_path: &str,
        project: &str,
    ) -> Result<Option<String>, StorageError> {
        self.inner
            .lock()
            .expect("storage lock poisoned")
            .get_file_hash(file_path, project)
    }

    fn get_all_file_hashes(&self, project: &str) -> Result<Vec<(String, String)>, StorageError> {
        self.inner
            .lock()
            .expect("storage lock poisoned")
            .get_all_file_hashes(project)
    }

    fn delete_project(&self, project_id: &str) -> Result<(), StorageError> {
        self.inner
            .lock()
            .expect("storage lock poisoned")
            .delete_project(project_id)
    }

    fn delete_file_nodes(&self, file_path: &str, project: &str) -> Result<(), StorageError> {
        self.inner
            .lock()
            .expect("storage lock poisoned")
            .delete_file_nodes(file_path, project)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{AsyncKit, StorageModule};
    use crate::model::{Edge, EdgeType, Language, Node, NodeLabel};

    /// Builds a StorageModule capability in-memory and returns it.
    fn build_storage() -> Arc<dyn Storage> {
        StorageModule::build_cap(&StorageConfig::in_memory()).expect("StorageModule::build_cap")
    }

    #[test]
    fn build_returns_send_sync_capability() {
        let cap = build_storage();
        fn _assert_send_sync<T: Send + Sync>(_: &T) {}
        _assert_send_sync(&cap);
    }

    #[test]
    fn capability_init_schema_is_idempotent() {
        let cap = build_storage();
        cap.init_schema().expect("first init_schema");
        cap.init_schema().expect("second init_schema");
    }

    #[test]
    fn capability_save_and_get_project() {
        let cap = build_storage();
        let node = Node::builder(NodeLabel::Project, "demo", "demo")
            .id("p1")
            .language(Language::Rust)
            .properties(serde_json::json!({
                "rootPath": "/repo/demo",
                "fileCount": 5,
                "indexedAt": 1_700_000_000,
            }))
            .build();
        cap.save_project(&node).expect("save_project");

        let rec = cap.get_project("p1").expect("get_project").unwrap();
        assert_eq!(rec.id, "p1");
        assert_eq!(rec.name, "demo");
        assert_eq!(rec.root_path, "/repo/demo");
    }

    #[test]
    fn capability_list_projects() {
        let cap = build_storage();
        assert!(cap.list_projects().expect("list_projects").is_empty());

        let node = Node::builder(NodeLabel::Project, "alpha", "alpha")
            .id("p1")
            .language(Language::Rust)
            .properties(serde_json::json!({"rootPath": "/", "fileCount": 0, "indexedAt": 0}))
            .build();
        cap.save_project(&node).expect("save_project");

        let projects = cap.list_projects().expect("list_projects");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "alpha");
    }

    #[test]
    fn capability_save_nodes_and_query_functions() {
        let cap = build_storage();
        let func = Node::builder(NodeLabel::Function, "main", "demo.main")
            .id("f1")
            .project("demo")
            .file_path("/src/main.rs")
            .start_line(1)
            .end_line(10)
            .signature("fn main()")
            .build();
        cap.save_nodes(&[func], NodeLabel::Function)
            .expect("save_nodes");

        let funcs = cap.query_functions("demo").expect("query_functions");
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "main");
    }

    #[test]
    fn capability_save_edges() {
        let cap = build_storage();
        let edge = Edge::builder("f1", "f2", EdgeType::Calls, "demo")
            .confidence(0.9)
            .start_line(5)
            .build();
        cap.save_edges(&[edge]).expect("save_edges");

        let rows = cap
            .query("MATCH (r:CodeRelation) RETURN r.type AS type;")
            .expect("query");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], serde_json::json!("CALLS"));
    }

    #[test]
    fn capability_delete_project() {
        let cap = build_storage();
        let node = Node::builder(NodeLabel::Project, "demo", "demo")
            .id("p1")
            .language(Language::Rust)
            .properties(serde_json::json!({"rootPath": "/", "fileCount": 0, "indexedAt": 0}))
            .build();
        cap.save_project(&node).expect("save_project");
        assert!(cap.get_project("p1").expect("get_project").is_some());

        cap.delete_project("p1").expect("delete_project");
        assert!(cap.get_project("p1").expect("get_project").is_none());
    }

    #[test]
    fn capability_file_hash_operations() {
        let cap = build_storage();
        let file = Node::builder(NodeLabel::File, "/src/main.rs", "/src/main.rs")
            .id("file_1")
            .project("demo")
            .file_path("/src/main.rs")
            .language(Language::Rust)
            .properties(serde_json::json!({"hash": "sha256:abc", "lineCount": 100}))
            .build();
        cap.save_nodes(&[file], NodeLabel::File)
            .expect("save_nodes");

        let hash = cap
            .get_file_hash("/src/main.rs", "demo")
            .expect("get_file_hash");
        assert_eq!(hash.as_deref(), Some("sha256:abc"));

        let all = cap
            .get_all_file_hashes("demo")
            .expect("get_all_file_hashes");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].0, "/src/main.rs");
        assert_eq!(all[0].1, "sha256:abc");
    }

    #[test]
    fn capability_delete_file_nodes() {
        let cap = build_storage();
        let func = Node::builder(NodeLabel::Function, "main", "demo.main")
            .id("f1")
            .project("demo")
            .file_path("/src/main.rs")
            .start_line(1)
            .end_line(10)
            .signature("fn main()")
            .build();
        cap.save_nodes(&[func], NodeLabel::Function)
            .expect("save_nodes");
        assert_eq!(
            cap.query_functions("demo").expect("query_functions").len(),
            1
        );

        cap.delete_file_nodes("/src/main.rs", "demo")
            .expect("delete_file_nodes");
        assert!(cap
            .query_functions("demo")
            .expect("query_functions")
            .is_empty());
    }

    #[test]
    fn capability_execute_and_query() {
        let cap = build_storage();
        cap.execute("CREATE (:Project {id: 'x', name: 'x', rootPath: '/', language: 'rust', fileCount: 0, indexedAt: 0});")
            .expect("execute");
        let rows = cap
            .query("MATCH (p:Project) RETURN p.name AS name;")
            .expect("query");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], serde_json::json!("x"));
    }

    /// Verify the full AsyncKit registration flow works end-to-end.
    #[tokio::test]
    async fn kit_registration_flow() {
        let mut kit = AsyncKit::new();
        kit.set_config(StorageConfig::in_memory());
        kit.register::<StorageModule>()
            .expect("register::<StorageModule>");
        let kit = kit.build().await.expect("build");

        assert!(kit.contains::<StorageModule>(), "StorageModule missing");

        let required = kit
            .require::<StorageModule>()
            .expect("require::<StorageModule>");
        required.init_schema().expect("init_schema");
    }
}
