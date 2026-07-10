// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Storage capability trait (T6/unified-architecture Phase 2, Task 2.3).
//!
//! Defines [`Storage`], the capability trait object stored in
//! [`Kit`](crate::kit::Kit) under [`StorageKey`](crate::kit::StorageKey). The
//! concrete implementation (Task 2.4) wraps a [`StorageConnection`] (or
//! [`Repository`]) and is registered as `Arc<dyn Storage>`.
//!
//! # Why operations, not a connection borrow
//!
//! [`StorageConnection`] is intentionally `!Clone` and owns the underlying
//! LadybugDB [`Database`](lbug::Database). A DB handle is not guaranteed to be
//! `Sync`, so the capability exposes **operations** (each taking `&self`)
//! rather than a `&StorageConnection` borrow. This lets the concrete impl use
//! interior mutability (e.g. `Mutex<StorageConnection>`) to satisfy
//! `Send + Sync` without constraining the storage layer's thread-safety.
//!
//! [`StorageConnection`]: super::StorageConnection
//! [`Repository`]: super::Repository

use crate::model::{Edge, Node, NodeLabel};

use super::connection::SchemaInitReport;
use super::error::StorageError;
use super::repository::{FunctionRecord, ProjectRecord};

/// Capability trait for the Storage subsystem (LadybugDB graph store).
///
/// Stored in [`Kit`](crate::kit::Kit) as `Arc<dyn Storage>` under
/// [`StorageKey`](crate::kit::StorageKey). Consumers (Indexer, Query, CLI
/// commands) call these operations instead of holding their own
/// [`StorageConnection`](super::StorageConnection).
///
/// Every method mirrors an existing method on [`StorageConnection`] or
/// [`Repository`](super::Repository); the concrete impl (Task 2.4) delegates
/// to them.
pub trait Storage: Send + Sync {
    /// Initializes the full CodeNexus schema. Idempotent.
    fn init_schema(&self) -> std::result::Result<SchemaInitReport, StorageError>;

    /// Executes a single Cypher statement that does not return rows (DDL/DML).
    fn execute(&self, cypher: &str) -> std::result::Result<(), StorageError>;

    /// Executes a Cypher query and returns all rows as JSON value vectors.
    fn query(&self, cypher: &str)
        -> std::result::Result<Vec<Vec<serde_json::Value>>, StorageError>;

    /// Saves a single `Project` node (label must be `NodeLabel::Project`).
    fn save_project(&self, node: &Node) -> std::result::Result<(), StorageError>;

    /// Bulk-saves nodes of a single label via CSV `COPY FROM` (ADR-014).
    fn save_nodes(&self, nodes: &[Node], label: NodeLabel)
        -> std::result::Result<(), StorageError>;

    /// Bulk-saves edges via CSV `COPY FROM` into the `CodeRelation` table.
    fn save_edges(&self, edges: &[Edge]) -> std::result::Result<(), StorageError>;

    /// Returns the project with the given id, or `None` if not found.
    fn get_project(&self, id: &str) -> std::result::Result<Option<ProjectRecord>, StorageError>;

    /// Lists all indexed projects, ordered by name.
    fn list_projects(&self) -> std::result::Result<Vec<ProjectRecord>, StorageError>;

    /// Returns all functions in the given project, ordered by qualified name.
    fn query_functions(
        &self,
        project: &str,
    ) -> std::result::Result<Vec<FunctionRecord>, StorageError>;

    /// Returns the stored hash for a file in the given project, or `None`.
    fn get_file_hash(
        &self,
        file_path: &str,
        project: &str,
    ) -> std::result::Result<Option<String>, StorageError>;

    /// Returns `(file_path, hash)` pairs for every file in the given project.
    fn get_all_file_hashes(
        &self,
        project: &str,
    ) -> std::result::Result<Vec<(String, String)>, StorageError>;

    /// Deletes a project and every node whose `project` column matches.
    fn delete_project(&self, project_id: &str) -> std::result::Result<(), StorageError>;

    /// Deletes every node whose `filePath` matches `file_path` in the project,
    /// plus orphaned `CodeRelation` rows referencing them.
    fn delete_file_nodes(
        &self,
        file_path: &str,
        project: &str,
    ) -> std::result::Result<(), StorageError>;
}

/// Compile-time assertion that `Storage` is object-safe and `Send + Sync`.
#[cfg(test)]
const _: () = {
    fn _assert_object_safe(_: &dyn Storage) {}
    fn _assert_send_sync<T: Send + Sync + ?Sized>() {}
    fn _check() {
        _assert_send_sync::<dyn Storage>();
        let _ = _assert_object_safe;
    }
};
