// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Repository pattern over [`StorageConnection`] (ADD §3.5).
//!
//! Provides CRUD operations on the code knowledge graph, abstracting away the
//! underlying Cypher queries and CSV bulk-loading mechanics. Callers interact
//! with domain types ([`Node`], [`Edge`]) and simple record structs
//! ([`ProjectRecord`], [`FunctionRecord`]) rather than raw query strings.
//!
//! # Multi-project isolation
//!
//! Every node table carries a `project` column (DDD §2.3). All repository
//! read/delete methods accept a `project` parameter and filter on it, ensuring
//! that data from one project never leaks into another (BR-INDEX-004).

use super::capability::Storage;
use super::connection::{SchemaInitReport, StorageConnection};
use super::error::{Result, StorageError};
use super::loader::{load_from_csv, write_csv_temp, write_edges_csv, write_nodes_csv};
use super::schema::{escape_cypher_string, escape_identifier, node_table_columns};
use crate::model::{Edge, Node, NodeLabel};

/// A simplified project record returned by [`Repository::get_project`] and
/// [`Repository::list_projects`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRecord {
    /// Project node id.
    pub id: String,
    /// Project display name.
    pub name: String,
    /// Repository root path.
    pub root_path: String,
    /// Primary source language.
    pub language: String,
    /// Number of indexed files.
    pub file_count: i64,
    /// Indexing timestamp (unix seconds).
    pub indexed_at: i64,
    /// Git commit hash at index time (empty if not a git repo).
    pub last_commit: String,
}

/// A simplified function record returned by [`Repository::query_functions`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionRecord {
    /// Function node id.
    pub id: String,
    /// Function name.
    pub name: String,
    /// Fully qualified name.
    pub qualified_name: String,
    /// Source file path.
    pub file_path: String,
    /// Start line (1-based).
    pub start_line: i64,
    /// End line (1-based, inclusive).
    pub end_line: i64,
    /// Function signature.
    pub signature: String,
}

/// Repository abstraction providing CRUD operations on the code knowledge
/// graph (ADD §3.5).
///
/// Wraps a [`StorageConnection`] and exposes domain-friendly methods. The
/// connection is owned by the repository; obtain one via
/// [`Repository::new`] or [`Repository::open`].
pub struct Repository {
    conn: StorageConnection,
}

impl Repository {
    /// Creates a new [`Repository`] wrapping the given connection.
    #[must_use]
    pub fn new(conn: StorageConnection) -> Self {
        Self { conn }
    }

    /// Opens (or creates) a LadybugDB database at `path`, initializes the
    /// schema, and returns a [`Repository`] over it.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let conn = StorageConnection::open(path)?;
        conn.init_schema()?;
        Ok(Self::new(conn))
    }

    /// Creates an in-memory repository (useful for tests).
    pub fn in_memory() -> Result<Self> {
        let conn = StorageConnection::in_memory()?;
        conn.init_schema()?;
        Ok(Self::new(conn))
    }

    /// Returns a reference to the underlying connection (for advanced callers).
    pub fn connection(&self) -> &StorageConnection {
        &self.conn
    }

    /// Initializes the schema on the underlying connection. Idempotent.
    ///
    /// Returns a [`SchemaInitReport`] describing any DDL statements that were
    /// skipped (unsupported by the linked LadybugDB build, or already present
    /// on re-init).
    pub fn init_schema(&self) -> Result<SchemaInitReport> {
        self.conn.init_schema()
    }

    /// Saves a single `Project` node.
    ///
    /// The node must have `label == NodeLabel::Project`; its `rootPath`,
    /// `fileCount`, and `indexedAt` are read from `node.properties`.
    pub fn save_project(&self, node: &Node) -> Result<()> {
        if node.label != NodeLabel::Project {
            return Err(StorageError::InvalidData(format!(
                "save_project requires NodeLabel::Project, got {}",
                node.label
            )));
        }
        let root_path = str_prop(node, "rootPath");
        let language = node
            .language
            .map(|l| l.to_string())
            .unwrap_or_default();
        let file_count = i64_prop(node, "fileCount");
        let indexed_at = i64_prop(node, "indexedAt");
        let last_commit = str_prop(node, "lastCommit");
        let cypher = format!(
            "CREATE (:Project {{id: '{}', name: '{}', rootPath: '{}', language: '{}', fileCount: {}, indexedAt: {}, lastCommit: '{}'}});",
            escape_cypher_string(&node.id),
            escape_cypher_string(&node.name),
            escape_cypher_string(&root_path),
            escape_cypher_string(&language),
            file_count,
            indexed_at,
            escape_cypher_string(&last_commit),
        );
        self.conn.execute(&cypher)
    }

    /// Bulk-saves nodes of a single label via CSV `COPY FROM` (ADR-014).
    ///
    /// Nodes are grouped by `label` because each label maps to a distinct
    /// table with a different column layout. The `label` field on each node
    /// is not checked — callers are responsible for passing a homogeneous
    /// slice.
    pub fn save_nodes(&self, nodes: &[Node], label: NodeLabel) -> Result<()> {
        if nodes.is_empty() {
            return Ok(());
        }
        let csv = write_nodes_csv(nodes, label);
        let table = label.table_name();
        let safe_id = nodes[0].id.replace(['/', '\\'], "_");
        let file_name = format!("{table}_{safe_id}.csv");
        let csv_path = write_csv_temp(&csv, &file_name)?;
        load_from_csv(&self.conn, table, &csv_path)
    }

    /// Bulk-saves edges via CSV `COPY FROM` into the `CodeRelation` table.
    pub fn save_edges(&self, edges: &[Edge]) -> Result<()> {
        if edges.is_empty() {
            return Ok(());
        }
        let csv = write_edges_csv(edges);
        let csv_path = write_csv_temp(&csv, "coderelation.csv")?;
        load_from_csv(&self.conn, "CodeRelation", &csv_path)
    }

    /// Returns the project with the given id, or `None` if not found.
    pub fn get_project(&self, id: &str) -> Result<Option<ProjectRecord>> {
        let cypher = format!(
            "MATCH (p:Project {{id: '{}'}}) RETURN p.id AS id, p.name AS name, p.rootPath AS rootPath, p.language AS language, p.fileCount AS fileCount, p.indexedAt AS indexedAt, p.lastCommit AS lastCommit;",
            escape_cypher_string(id),
        );
        let rows = self.conn.query(&cypher)?;
        Ok(rows.into_iter().next().map(row_to_project))
    }

    /// Lists all indexed projects.
    pub fn list_projects(&self) -> Result<Vec<ProjectRecord>> {
        let cypher = "MATCH (p:Project) RETURN p.id AS id, p.name AS name, p.rootPath AS rootPath, p.language AS language, p.fileCount AS fileCount, p.indexedAt AS indexedAt, p.lastCommit AS lastCommit ORDER BY p.name;";
        let rows = self.conn.query(cypher)?;
        Ok(rows.into_iter().map(row_to_project).collect())
    }

    /// Deletes a project and every node whose `project` column matches its id.
    ///
    /// Also deletes `CodeRelation` rows belonging to the project. This
    /// implements the multi-project isolation cleanup (BR-INDEX-004).
    pub fn delete_project(&self, project_id: &str) -> Result<()> {
        let escaped = escape_cypher_string(project_id);
        // Delete CodeRelation rows for the project.
        let cypher = format!(
            "MATCH (r:CodeRelation) WHERE r.project = '{escaped}' DELETE r;"
        );
        self.conn.execute(&cypher)?;
        // Delete nodes from every node table that has a `project` column.
        // Project itself is matched by id; all other tables by `project`.
        for label in NodeLabel::all() {
            let table = escape_identifier(label.table_name());
            let cypher = if label == NodeLabel::Project {
                format!("MATCH (n:{table} {{id: '{escaped}'}}) DELETE n;")
            } else {
                format!("MATCH (n:{table}) WHERE n.project = '{escaped}' DELETE n;")
            };
            // Some tables may not exist or the column may be missing; treat
            // those as non-fatal.
            if let Err(err) = self.conn.execute(&cypher) {
                let msg = err.to_string();
                if !msg.contains("does not exist") && !msg.contains("no such") {
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    /// Returns the stored hash for a file in the given project, or `None`.
    ///
    /// Used by the incremental indexer to detect changes (BR-INDEX-001).
    pub fn get_file_hash(&self, file_path: &str, project: &str) -> Result<Option<String>> {
        let cypher = format!(
            "MATCH (f:File) WHERE f.filePath = '{}' AND f.project = '{}' RETURN f.hash AS hash;",
            escape_cypher_string(file_path),
            escape_cypher_string(project),
        );
        let rows = self.conn.query(&cypher)?;
        Ok(rows
            .into_iter()
            .next()
            .and_then(|row| row.into_iter().next())
            .and_then(|v| v.as_str().map(String::from)))
    }

    /// Returns `(file_path, hash)` pairs for every file in the given project.
    ///
    /// Used by the incremental indexer to compute the diff set
    /// (added/changed/deleted) in a single query.
    pub fn get_all_file_hashes(&self, project: &str) -> Result<Vec<(String, String)>> {
        let cypher = format!(
            "MATCH (f:File) WHERE f.project = '{}' RETURN f.filePath AS filePath, f.hash AS hash;",
            escape_cypher_string(project),
        );
        let rows = self.conn.query(&cypher)?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let path = row
                .first()
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_default();
            let hash = row
                .get(1)
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_default();
            out.push((path, hash));
        }
        Ok(out)
    }

    /// Deletes every node whose `filePath` matches `file_path` in the given
    /// project, across all node tables that carry a `filePath` column.
    ///
    /// Also deletes `CodeRelation` rows whose `source` or `target` references
    /// a deleted node. Used by the incremental indexer when a file is removed
    /// or re-parsed (BR-INDEX-002).
    pub fn delete_file_nodes(&self, file_path: &str, project: &str) -> Result<()> {
        let path_escaped = escape_cypher_string(file_path);
        let proj_escaped = escape_cypher_string(project);
        // Collect the ids of nodes belonging to this file so we can clean up
        // CodeRelation rows referencing them.
        let mut orphan_ids: Vec<String> = Vec::new();
        for label in NodeLabel::all() {
            if label == NodeLabel::Project {
                continue;
            }
            // Skip tables without a `filePath` column (e.g. Process, Community,
            // Embedding) — querying `n.filePath` against them raises a binder
            // error. Deterministic column check per Rule 5 instead of relying
            // on error-message matching.
            if !node_table_columns(label).contains(&"filePath") {
                continue;
            }
            let table = escape_identifier(label.table_name());
            // Only tables with a filePath column are affected.
            let select = format!(
                "MATCH (n:{table}) WHERE n.filePath = '{path_escaped}' AND n.project = '{proj_escaped}' RETURN n.id AS id;"
            );
            if let Ok(rows) = self.conn.query(&select) {
                for row in rows {
                    if let Some(id) = row
                        .first()
                        .and_then(|v| v.as_str())
                        .map(String::from)
                    {
                        orphan_ids.push(id);
                    }
                }
            }
            let delete = format!(
                "MATCH (n:{table}) WHERE n.filePath = '{path_escaped}' AND n.project = '{proj_escaped}' DELETE n;"
            );
            if let Err(err) = self.conn.execute(&delete) {
                let msg = err.to_string();
                if !msg.contains("does not exist") && !msg.contains("no such") {
                    return Err(err);
                }
            }
        }
        // Delete CodeRelation rows referencing the orphaned node ids.
        if !orphan_ids.is_empty() {
            let id_list = orphan_ids
                .iter()
                .map(|id| format!("'{}'", escape_cypher_string(id)))
                .collect::<Vec<_>>()
                .join(", ");
            let cypher = format!(
                "MATCH (r:CodeRelation) WHERE r.source IN [{id_list}] OR r.target IN [{id_list}] DELETE r;"
            );
            if let Err(err) = self.conn.execute(&cypher) {
                let msg = err.to_string();
                if !msg.contains("does not exist") && !msg.contains("no such") {
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    /// Batch version of [`delete_file_nodes`](Self::delete_file_nodes) that
    /// removes nodes for multiple file paths in a single pass over the node
    /// labels, instead of one pass per file.
    ///
    /// # Performance motivation
    ///
    /// `delete_file_nodes` runs ~21 Cypher queries per file (one SELECT +
    /// one DELETE per label with a `filePath` column, plus one CodeRelation
    /// DELETE). For an incremental re-index touching 500 of 1000 files that
    /// is 10 500 queries — the dominant cost behind the
    /// `incremental_500_of_1000` bench SLO violation (33 files/s vs the
    /// PRD ≥100 files/s target).
    ///
    /// This batch variant collapses the per-file loop into a single
    /// `WHERE n.filePath IN [...]` pass, keeping the query count fixed at
    /// ~21 regardless of how many files are deleted.
    ///
    /// # Arguments
    ///
    /// * `paths` - Relative file paths whose nodes should be removed.
    /// * `project` - Project id isolating the delete (BR-INDEX-004).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if a non-"table missing" delete fails. Missing
    /// tables are tolerated (same tolerance as `delete_file_nodes`) so the
    /// batch path works on schemas that have not yet created every label
    /// table.
    pub fn delete_file_nodes_batch(&self, paths: &[String], project: &str) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let proj_escaped = escape_cypher_string(project);
        let path_list = paths
            .iter()
            .map(|p| format!("'{}'", escape_cypher_string(p)))
            .collect::<Vec<_>>()
            .join(", ");

        let mut orphan_ids: Vec<String> = Vec::new();
        for label in NodeLabel::all() {
            if label == NodeLabel::Project {
                continue;
            }
            if !node_table_columns(label).contains(&"filePath") {
                continue;
            }
            let table = escape_identifier(label.table_name());
            let select = format!(
                "MATCH (n:{table}) WHERE n.filePath IN [{path_list}] AND n.project = '{proj_escaped}' RETURN n.id AS id;"
            );
            if let Ok(rows) = self.conn.query(&select) {
                for row in rows {
                    if let Some(id) = row
                        .first()
                        .and_then(|v| v.as_str())
                        .map(String::from)
                    {
                        orphan_ids.push(id);
                    }
                }
            }
            let delete = format!(
                "MATCH (n:{table}) WHERE n.filePath IN [{path_list}] AND n.project = '{proj_escaped}' DELETE n;"
            );
            if let Err(err) = self.conn.execute(&delete) {
                let msg = err.to_string();
                if !msg.contains("does not exist") && !msg.contains("no such") {
                    return Err(err);
                }
            }
        }
        if !orphan_ids.is_empty() {
            let id_list = orphan_ids
                .iter()
                .map(|id| format!("'{}'", escape_cypher_string(id)))
                .collect::<Vec<_>>()
                .join(", ");
            let cypher = format!(
                "MATCH (r:CodeRelation) WHERE r.source IN [{id_list}] OR r.target IN [{id_list}] DELETE r;"
            );
            if let Err(err) = self.conn.execute(&cypher) {
                let msg = err.to_string();
                if !msg.contains("does not exist") && !msg.contains("no such") {
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    /// Returns all functions in the given project.
    ///
    /// Functions are ordered by `qualifiedName` for deterministic output.
    pub fn query_functions(&self, project: &str) -> Result<Vec<FunctionRecord>> {
        let cypher = format!(
            "MATCH (f:Function) WHERE f.project = '{}' RETURN f.id AS id, f.name AS name, f.qualifiedName AS qualifiedName, f.filePath AS filePath, f.startLine AS startLine, f.endLine AS endLine, f.signature AS signature ORDER BY f.qualifiedName;",
            escape_cypher_string(project),
        );
        let rows = self.conn.query(&cypher)?;
        Ok(rows.into_iter().map(row_to_function).collect())
    }
}

/// Delegates the [`Storage`] capability trait to a bare [`Repository`].
///
/// This exists so callers which need a FRESH `Repository` (opened after
/// writes occurred) can pass it to APIs expecting `&dyn Storage` — most
/// notably [`QualityChecker`](super::QualityChecker) in `index_cmd::run`.
/// The Kit's storage capability wraps a `Repository` opened at boot; using
/// it after indexing would read a stale MVCC snapshot and risk "checkpoint
/// interference on drop" (the stale Repository's destructor may flush its
/// empty view over the indexer's writes). Opening a new `Repository` and
/// passing it as `&dyn Storage` avoids both problems.
///
/// # Thread-safety
///
/// This impl does NOT lock a mutex (unlike the Kit's storage capability,
/// which wraps `Repository` in a `Mutex`). It is safe for single-threaded
/// consumers such as `QualityChecker`. Multi-threaded consumers must use
/// the Kit's mutex-guarded capability instead.
impl Storage for Repository {
    fn init_schema(&self) -> std::result::Result<SchemaInitReport, StorageError> {
        Repository::init_schema(self)
    }

    fn execute(&self, cypher: &str) -> std::result::Result<(), StorageError> {
        self.connection().execute(cypher)
    }

    fn query(
        &self,
        cypher: &str,
    ) -> std::result::Result<Vec<Vec<serde_json::Value>>, StorageError> {
        self.connection().query(cypher)
    }

    fn save_project(&self, node: &Node) -> std::result::Result<(), StorageError> {
        Repository::save_project(self, node)
    }

    fn save_nodes(
        &self,
        nodes: &[Node],
        label: NodeLabel,
    ) -> std::result::Result<(), StorageError> {
        Repository::save_nodes(self, nodes, label)
    }

    fn save_edges(&self, edges: &[Edge]) -> std::result::Result<(), StorageError> {
        Repository::save_edges(self, edges)
    }

    fn get_project(
        &self,
        id: &str,
    ) -> std::result::Result<Option<ProjectRecord>, StorageError> {
        Repository::get_project(self, id)
    }

    fn list_projects(&self) -> std::result::Result<Vec<ProjectRecord>, StorageError> {
        Repository::list_projects(self)
    }

    fn query_functions(
        &self,
        project: &str,
    ) -> std::result::Result<Vec<FunctionRecord>, StorageError> {
        Repository::query_functions(self, project)
    }

    fn get_file_hash(
        &self,
        file_path: &str,
        project: &str,
    ) -> std::result::Result<Option<String>, StorageError> {
        Repository::get_file_hash(self, file_path, project)
    }

    fn get_all_file_hashes(
        &self,
        project: &str,
    ) -> std::result::Result<Vec<(String, String)>, StorageError> {
        Repository::get_all_file_hashes(self, project)
    }

    fn delete_project(&self, project_id: &str) -> std::result::Result<(), StorageError> {
        Repository::delete_project(self, project_id)
    }

    fn delete_file_nodes(
        &self,
        file_path: &str,
        project: &str,
    ) -> std::result::Result<(), StorageError> {
        Repository::delete_file_nodes(self, file_path, project)
    }
}

/// Extracts a string property from a node's `properties` JSON.
fn str_prop(node: &Node, key: &str) -> String {
    node.properties
        .get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_default()
}

/// Extracts an integer property from a node's `properties` JSON.
fn i64_prop(node: &Node, key: &str) -> i64 {
    node.properties
        .get(key)
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
}

/// Converts a query row into a [`ProjectRecord`].
fn row_to_project(row: Vec<serde_json::Value>) -> ProjectRecord {
    let get_str = |idx: usize| -> String {
        row.get(idx)
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_default()
    };
    let get_i64 = |idx: usize| -> i64 {
        row.get(idx)
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
    };
    ProjectRecord {
        id: get_str(0),
        name: get_str(1),
        root_path: get_str(2),
        language: get_str(3),
        file_count: get_i64(4),
        indexed_at: get_i64(5),
        last_commit: get_str(6),
    }
}

/// Converts a query row into a [`FunctionRecord`].
fn row_to_function(row: Vec<serde_json::Value>) -> FunctionRecord {
    let get_str = |idx: usize| -> String {
        row.get(idx)
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_default()
    };
    let get_i64 = |idx: usize| -> i64 {
        row.get(idx)
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
    };
    FunctionRecord {
        id: get_str(0),
        name: get_str(1),
        qualified_name: get_str(2),
        file_path: get_str(3),
        start_line: get_i64(4),
        end_line: get_i64(5),
        signature: get_str(6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EdgeType, Language};

    /// Creates a fresh in-memory repository with the schema initialized.
    fn fresh_repo() -> Repository {
        Repository::in_memory().expect("in_memory repository")
    }

    /// Builds a sample Project node.
    fn sample_project(id: &str, name: &str) -> Node {
        Node::builder(NodeLabel::Project, name, name)
            .id(id)
            .language(Language::Rust)
            .properties(serde_json::json!({
                "rootPath": "/repo/".to_string() + name,
                "fileCount": 10,
                "indexedAt": 1_700_000_000,
            }))
            .build()
    }

    /// Builds a sample File node.
    fn sample_file(id: &str, project: &str, path: &str, hash: &str) -> Node {
        Node::builder(NodeLabel::File, path, path)
            .id(id)
            .project(project)
            .file_path(path)
            .language(Language::Rust)
            .properties(serde_json::json!({"hash": hash, "lineCount": 100}))
            .build()
    }

    /// Builds a sample Function node.
    fn sample_function(id: &str, project: &str, name: &str, qn: &str) -> Node {
        Node::builder(NodeLabel::Function, name, qn)
            .id(id)
            .project(project)
            .file_path("/src/main.rs")
            .start_line(1)
            .end_line(10)
            .signature("fn main()")
            .build()
    }

    // --- save_project / get_project ---

    #[test]
    fn save_project_persists_node() {
        let repo = fresh_repo();
        let node = sample_project("proj_1", "demo");
        repo.save_project(&node).expect("save_project");

        let fetched = repo.get_project("proj_1").expect("get_project");
        assert!(fetched.is_some());
        let rec = fetched.unwrap();
        assert_eq!(rec.id, "proj_1");
        assert_eq!(rec.name, "demo");
        assert_eq!(rec.root_path, "/repo/demo");
        assert_eq!(rec.language, "rust");
        assert_eq!(rec.file_count, 10);
        assert_eq!(rec.indexed_at, 1_700_000_000);
    }

    #[test]
    fn get_project_returns_none_when_missing() {
        let repo = fresh_repo();
        let fetched = repo.get_project("does_not_exist").expect("get_project");
        assert!(fetched.is_none());
    }

    #[test]
    fn save_project_rejects_non_project_label() {
        let repo = fresh_repo();
        let node = Node::builder(NodeLabel::Function, "f", "qn")
            .id("f1")
            .build();
        let err = repo.save_project(&node);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("Project"));
    }

    #[test]
    fn save_project_escapes_single_quotes_in_name() {
        let repo = fresh_repo();
        let node = Node::builder(NodeLabel::Project, "it's demo", "qn")
            .id("p1")
            .properties(serde_json::json!({"rootPath": "/", "fileCount": 0, "indexedAt": 0}))
            .build();
        repo.save_project(&node).expect("save_project");

        let rec = repo.get_project("p1").unwrap().unwrap();
        assert_eq!(rec.name, "it's demo");
    }

    // --- list_projects ---

    #[test]
    fn list_projects_returns_all_projects_ordered_by_name() {
        let repo = fresh_repo();
        repo.save_project(&sample_project("p1", "zeta")).unwrap();
        repo.save_project(&sample_project("p2", "alpha")).unwrap();
        repo.save_project(&sample_project("p3", "mid")).unwrap();

        let projects = repo.list_projects().expect("list_projects");
        assert_eq!(projects.len(), 3);
        assert_eq!(projects[0].name, "alpha");
        assert_eq!(projects[1].name, "mid");
        assert_eq!(projects[2].name, "zeta");
    }

    #[test]
    fn list_projects_empty_when_none() {
        let repo = fresh_repo();
        let projects = repo.list_projects().expect("list_projects");
        assert!(projects.is_empty());
    }

    // --- save_nodes / save_edges ---

    #[test]
    fn save_nodes_loads_function_nodes() {
        let repo = fresh_repo();
        let nodes = vec![
            sample_function("f1", "demo", "main", "demo.main"),
            sample_function("f2", "demo", "helper", "demo.helper"),
        ];
        repo.save_nodes(&nodes, NodeLabel::Function).expect("save_nodes");

        let funcs = repo.query_functions("demo").expect("query_functions");
        assert_eq!(funcs.len(), 2);
        assert_eq!(funcs[0].name, "helper");
        assert_eq!(funcs[1].name, "main");
    }

    #[test]
    fn save_nodes_empty_slice_is_noop() {
        let repo = fresh_repo();
        let result = repo.save_nodes(&[], NodeLabel::Function);
        assert!(result.is_ok());
    }

    #[test]
    fn save_nodes_handles_macro_label() {
        let repo = fresh_repo();
        let node = Node::builder(NodeLabel::Macro, "M", "demo.M")
            .id("m1")
            .project("demo")
            .start_line(1)
            .end_line(2)
            .signature("#define M x")
            .properties(serde_json::json!({"content": "#define M x"}))
            .build();
        repo.save_nodes(&[node], NodeLabel::Macro).expect("save_nodes Macro");

        let rows = repo
            .connection()
            .query("MATCH (m:`Macro`) RETURN m.name AS name;")
            .expect("query Macro");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], serde_json::json!("M"));
    }

    #[test]
    fn save_edges_loads_code_relations() {
        let repo = fresh_repo();
        let edges = vec![
            Edge::builder("f1", "f2", EdgeType::Calls, "demo")
                .confidence(0.9)
                .start_line(5)
                .build(),
            Edge::builder("f2", "f3", EdgeType::Calls, "demo")
                .confidence(0.8)
                .build(),
        ];
        repo.save_edges(&edges).expect("save_edges");

        let rows = repo
            .connection()
            .query("MATCH (r:CodeRelation) RETURN r.type AS type ORDER BY r.id;")
            .expect("query CodeRelation");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], serde_json::json!("CALLS"));
    }

    #[test]
    fn save_edges_empty_slice_is_noop() {
        let repo = fresh_repo();
        let result = repo.save_edges(&[]);
        assert!(result.is_ok());
    }

    // --- delete_project ---

    #[test]
    fn delete_project_removes_project_and_related_nodes() {
        let repo = fresh_repo();
        repo.save_project(&sample_project("demo", "demo")).unwrap();
        repo.save_nodes(
            &[sample_function("f1", "demo", "main", "demo.main")],
            NodeLabel::Function,
        )
        .unwrap();
        repo.save_nodes(
            &[sample_file("file_1", "demo", "/src/main.rs", "abc")],
            NodeLabel::File,
        )
        .unwrap();

        // Sanity check: data is present.
        assert!(repo.get_project("demo").unwrap().is_some());
        assert_eq!(repo.query_functions("demo").unwrap().len(), 1);

        repo.delete_project("demo").expect("delete_project");

        // Project and its nodes are gone.
        assert!(repo.get_project("demo").unwrap().is_none());
        assert!(repo.query_functions("demo").unwrap().is_empty());

        let file_rows = repo
            .connection()
            .query("MATCH (f:File) RETURN f.id AS id;")
            .unwrap();
        assert!(file_rows.is_empty());
    }

    #[test]
    fn delete_project_only_removes_specified_project() {
        let repo = fresh_repo();
        repo.save_project(&sample_project("alpha", "alpha")).unwrap();
        repo.save_project(&sample_project("beta", "beta")).unwrap();
        repo.save_nodes(
            &[sample_function("f1", "alpha", "main", "alpha.main")],
            NodeLabel::Function,
        )
        .unwrap();
        repo.save_nodes(
            &[sample_function("f2", "beta", "main", "beta.main")],
            NodeLabel::Function,
        )
        .unwrap();

        repo.delete_project("alpha").expect("delete_project");

        // alpha is gone, beta remains.
        assert!(repo.get_project("alpha").unwrap().is_none());
        assert!(repo.get_project("beta").unwrap().is_some());
        assert!(repo.query_functions("alpha").unwrap().is_empty());
        assert_eq!(repo.query_functions("beta").unwrap().len(), 1);
    }

    #[test]
    fn delete_project_nonexistent_is_noop() {
        let repo = fresh_repo();
        // Should not error even though the project doesn't exist.
        let result = repo.delete_project("never_existed");
        assert!(result.is_ok());
    }

    // --- get_file_hash / get_all_file_hashes ---

    #[test]
    fn get_file_hash_returns_stored_hash() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_file("file_1", "demo", "/src/main.rs", "sha256:abc")],
            NodeLabel::File,
        )
        .unwrap();

        let hash = repo
            .get_file_hash("/src/main.rs", "demo")
            .expect("get_file_hash");
        assert_eq!(hash.as_deref(), Some("sha256:abc"));
    }

    #[test]
    fn get_file_hash_returns_none_when_missing() {
        let repo = fresh_repo();
        let hash = repo
            .get_file_hash("/nope.rs", "demo")
            .expect("get_file_hash");
        assert!(hash.is_none());
    }

    #[test]
    fn get_file_hash_isolates_by_project() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_file("f1", "alpha", "/src/main.rs", "hash_alpha")],
            NodeLabel::File,
        )
        .unwrap();
        repo.save_nodes(
            &[sample_file("f2", "beta", "/src/main.rs", "hash_beta")],
            NodeLabel::File,
        )
        .unwrap();

        // Same path, different projects → different hashes.
        assert_eq!(
            repo.get_file_hash("/src/main.rs", "alpha").unwrap().as_deref(),
            Some("hash_alpha")
        );
        assert_eq!(
            repo.get_file_hash("/src/main.rs", "beta").unwrap().as_deref(),
            Some("hash_beta")
        );
    }

    #[test]
    fn get_all_file_hashes_returns_all_files_for_project() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_file("f1", "demo", "/a.rs", "hash_a"),
                sample_file("f2", "demo", "/b.rs", "hash_b"),
                sample_file("f3", "demo", "/c.rs", "hash_c"),
            ],
            NodeLabel::File,
        )
        .unwrap();
        // A file in another project should not appear.
        repo.save_nodes(
            &[sample_file("f4", "other", "/d.rs", "hash_d")],
            NodeLabel::File,
        )
        .unwrap();

        let hashes = repo
            .get_all_file_hashes("demo")
            .expect("get_all_file_hashes");
        assert_eq!(hashes.len(), 3);
        let paths: Vec<&str> = hashes.iter().map(|(p, _)| p.as_str()).collect();
        assert!(paths.contains(&"/a.rs"));
        assert!(paths.contains(&"/b.rs"));
        assert!(paths.contains(&"/c.rs"));
        assert!(!paths.contains(&"/d.rs"));
    }

    #[test]
    fn get_all_file_hashes_empty_when_no_files() {
        let repo = fresh_repo();
        let hashes = repo
            .get_all_file_hashes("demo")
            .expect("get_all_file_hashes");
        assert!(hashes.is_empty());
    }

    // --- delete_file_nodes ---

    #[test]
    fn delete_file_nodes_removes_nodes_for_file() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_function("f1", "demo", "main", "demo.main"),
                sample_function("f2", "demo", "helper", "demo.helper"),
            ],
            NodeLabel::Function,
        )
        .unwrap();

        // Both functions are in /src/main.rs; deleting that file removes both.
        repo.delete_file_nodes("/src/main.rs", "demo")
            .expect("delete_file_nodes");
        assert!(repo.query_functions("demo").unwrap().is_empty());
    }

    #[test]
    fn delete_file_nodes_isolates_by_project() {
        let repo = fresh_repo();
        // Same path, different projects.
        repo.save_nodes(
            &[sample_function("f1", "alpha", "main", "alpha.main")],
            NodeLabel::Function,
        )
        .unwrap();
        repo.save_nodes(
            &[sample_function("f2", "beta", "main", "beta.main")],
            NodeLabel::Function,
        )
        .unwrap();

        repo.delete_file_nodes("/src/main.rs", "alpha")
            .expect("delete_file_nodes");
        assert!(repo.query_functions("alpha").unwrap().is_empty());
        assert_eq!(repo.query_functions("beta").unwrap().len(), 1);
    }

    #[test]
    fn delete_file_nodes_also_removes_related_edges() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_function("f1", "demo", "main", "demo.main"),
                sample_function("f2", "demo", "helper", "demo.helper"),
            ],
            NodeLabel::Function,
        )
        .unwrap();
        repo.save_edges(&[Edge::builder("f1", "f2", EdgeType::Calls, "demo")
            .start_line(3)
            .build()])
        .unwrap();

        // Sanity: edge exists.
        let rows = repo
            .connection()
            .query("MATCH (r:CodeRelation) RETURN count(r) AS cnt;")
            .unwrap();
        assert_eq!(rows[0][0], serde_json::json!(1));

        repo.delete_file_nodes("/src/main.rs", "demo")
            .expect("delete_file_nodes");

        // Edge referencing the deleted nodes is gone.
        let rows = repo
            .connection()
            .query("MATCH (r:CodeRelation) RETURN count(r) AS cnt;")
            .unwrap();
        assert_eq!(rows[0][0], serde_json::json!(0));
    }

    #[test]
    fn delete_file_nodes_nonexistent_is_noop() {
        let repo = fresh_repo();
        let result = repo.delete_file_nodes("/nope.rs", "demo");
        assert!(result.is_ok());
    }

    // --- query_functions ---

    #[test]
    fn query_functions_returns_functions_ordered_by_qn() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_function("f1", "demo", "zeta", "demo.zeta"),
                sample_function("f2", "demo", "alpha", "demo.alpha"),
                sample_function("f3", "demo", "mid", "demo.mid"),
            ],
            NodeLabel::Function,
        )
        .unwrap();

        let funcs = repo.query_functions("demo").expect("query_functions");
        assert_eq!(funcs.len(), 3);
        assert_eq!(funcs[0].qualified_name, "demo.alpha");
        assert_eq!(funcs[1].qualified_name, "demo.mid");
        assert_eq!(funcs[2].qualified_name, "demo.zeta");
    }

    #[test]
    fn query_functions_isolates_by_project() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function("f1", "alpha", "main", "alpha.main")],
            NodeLabel::Function,
        )
        .unwrap();
        repo.save_nodes(
            &[sample_function("f2", "beta", "main", "beta.main")],
            NodeLabel::Function,
        )
        .unwrap();

        assert_eq!(repo.query_functions("alpha").unwrap().len(), 1);
        assert_eq!(repo.query_functions("beta").unwrap().len(), 1);
        assert_eq!(repo.query_functions("gamma").unwrap().len(), 0);
    }

    #[test]
    fn query_functions_empty_when_none() {
        let repo = fresh_repo();
        let funcs = repo.query_functions("demo").expect("query_functions");
        assert!(funcs.is_empty());
    }

    // --- multi-project isolation (BR-INDEX-004) ---

    #[test]
    fn multi_project_isolation_br_index_004() {
        // Two projects coexist; querying/deleting one never affects the other.
        let repo = fresh_repo();
        repo.save_project(&sample_project("alpha", "alpha")).unwrap();
        repo.save_project(&sample_project("beta", "beta")).unwrap();

        repo.save_nodes(
            &[
                sample_function("a1", "alpha", "main", "alpha.main"),
                sample_function("a2", "alpha", "util", "alpha.util"),
            ],
            NodeLabel::Function,
        )
        .unwrap();
        repo.save_nodes(
            &[
                sample_function("b1", "beta", "main", "beta.main"),
                sample_function("b2", "beta", "util", "beta.util"),
                sample_function("b3", "beta", "extra", "beta.extra"),
            ],
            NodeLabel::Function,
        )
        .unwrap();

        // Each project sees only its own functions.
        assert_eq!(repo.query_functions("alpha").unwrap().len(), 2);
        assert_eq!(repo.query_functions("beta").unwrap().len(), 3);

        // Deleting alpha leaves beta untouched.
        repo.delete_project("alpha").expect("delete_project alpha");
        assert!(repo.get_project("alpha").unwrap().is_none());
        assert!(repo.get_project("beta").unwrap().is_some());
        assert_eq!(repo.query_functions("alpha").unwrap().len(), 0);
        assert_eq!(repo.query_functions("beta").unwrap().len(), 3);

        // list_projects reflects the deletion.
        let projects = repo.list_projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, "beta");
    }

    // --- open / connection / init_schema ---

    #[test]
    fn open_creates_repository_with_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("testdb");
        let repo = Repository::open(&path).expect("Repository::open");
        std::mem::forget(dir);

        // Schema is initialized: querying an empty Project table works.
        let projects = repo.list_projects().expect("list_projects");
        assert!(projects.is_empty());
    }

    #[test]
    fn connection_returns_underlying_handle() {
        let repo = fresh_repo();
        let _ = repo.connection();
    }

    #[test]
    fn init_schema_is_idempotent() {
        let repo = fresh_repo();
        repo.init_schema().expect("first init_schema");
        repo.init_schema().expect("second init_schema");
    }

    // --- helpers ---

    #[test]
    fn project_record_equality() {
        let a = ProjectRecord {
            id: "p1".into(),
            name: "demo".into(),
            root_path: "/".into(),
            language: "rust".into(),
            file_count: 1,
            indexed_at: 2,
            last_commit: "abc123".into(),
        };
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn function_record_debug() {
        let rec = FunctionRecord {
            id: "f1".into(),
            name: "main".into(),
            qualified_name: "demo.main".into(),
            file_path: "/src/main.rs".into(),
            start_line: 1,
            end_line: 10,
            signature: "fn main()".into(),
        };
        let s = format!("{rec:?}");
        assert!(s.contains("FunctionRecord"));
        assert!(s.contains("main"));
    }

    // --- Storage trait impl on Repository (delegation coverage) ---

    #[test]
    fn repository_impl_storage_init_schema_works() {
        let repo = fresh_repo();
        let storage: &dyn Storage = &repo;
        let report = storage.init_schema().expect("init_schema via Storage trait");
        // Idempotent — schema already initialized by fresh_repo.
        let _ = report.skipped_count;
    }

    #[test]
    fn repository_impl_storage_execute_and_query() {
        let repo = fresh_repo();
        let storage: &dyn Storage = &repo;
        storage
            .execute("CREATE (:Project {id: 'p1', name: 'demo', rootPath: '/', language: 'rust', fileCount: 0, indexedAt: 0, lastCommit: ''});")
            .expect("execute via Storage trait");
        let rows = storage.query("MATCH (p:Project) RETURN p.name AS name;").expect("query via Storage trait");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0].as_str(), Some("demo"));
    }

    #[test]
    fn repository_impl_storage_save_and_get_project() {
        let repo = fresh_repo();
        let storage: &dyn Storage = &repo;
        storage.save_project(&sample_project("demo", "demo")).expect("save_project");
        let proj = storage.get_project("demo").expect("get_project");
        assert!(proj.is_some());
        assert_eq!(proj.unwrap().name, "demo");
    }

    #[test]
    fn repository_impl_storage_list_projects() {
        let repo = fresh_repo();
        let storage: &dyn Storage = &repo;
        storage.save_project(&sample_project("alpha", "alpha")).unwrap();
        storage.save_project(&sample_project("beta", "beta")).unwrap();
        let projects = storage.list_projects().expect("list_projects");
        assert_eq!(projects.len(), 2);
    }

    #[test]
    fn repository_impl_storage_save_nodes_and_query_functions() {
        let repo = fresh_repo();
        let storage: &dyn Storage = &repo;
        storage.save_project(&sample_project("demo", "demo")).unwrap();
        let nodes = vec![sample_function("f1", "demo", "main", "demo.main")];
        storage.save_nodes(&nodes, NodeLabel::Function).expect("save_nodes");
        let funcs = storage.query_functions("demo").expect("query_functions");
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].name, "main");
    }

    #[test]
    fn repository_impl_storage_save_edges() {
        let repo = fresh_repo();
        let storage: &dyn Storage = &repo;
        let edge = Edge::builder("s", "t", EdgeType::Calls, "demo")
            .start_line(1)
            .build();
        storage.save_edges(&[edge]).expect("save_edges");
        let rows = storage
            .query("MATCH (r:CodeRelation) RETURN r.source AS src;")
            .expect("query edges");
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn repository_impl_storage_get_file_hash() {
        let repo = fresh_repo();
        let storage: &dyn Storage = &repo;
        storage.save_project(&sample_project("demo", "demo")).unwrap();
        storage
            .save_nodes(&[sample_file("f1", "demo", "/src/main.rs", "abc123")], NodeLabel::File)
            .unwrap();
        let hash = storage.get_file_hash("/src/main.rs", "demo").expect("get_file_hash");
        assert_eq!(hash.as_deref(), Some("abc123"));
    }

    #[test]
    fn repository_impl_storage_get_all_file_hashes() {
        let repo = fresh_repo();
        let storage: &dyn Storage = &repo;
        storage.save_project(&sample_project("demo", "demo")).unwrap();
        storage
            .save_nodes(&[sample_file("f1", "demo", "/src/a.rs", "hash1")], NodeLabel::File)
            .unwrap();
        storage
            .save_nodes(&[sample_file("f2", "demo", "/src/b.rs", "hash2")], NodeLabel::File)
            .unwrap();
        let hashes = storage.get_all_file_hashes("demo").expect("get_all_file_hashes");
        assert_eq!(hashes.len(), 2);
    }

    #[test]
    fn repository_impl_storage_delete_project() {
        let repo = fresh_repo();
        let storage: &dyn Storage = &repo;
        storage.save_project(&sample_project("demo", "demo")).unwrap();
        storage.delete_project("demo").expect("delete_project");
        assert!(storage.get_project("demo").unwrap().is_none());
    }

    #[test]
    fn repository_impl_storage_delete_file_nodes() {
        let repo = fresh_repo();
        let storage: &dyn Storage = &repo;
        storage.save_project(&sample_project("demo", "demo")).unwrap();
        storage
            .save_nodes(&[sample_file("f1", "demo", "/src/main.rs", "abc")], NodeLabel::File)
            .unwrap();
        storage
            .save_nodes(&[sample_function("f1", "demo", "main", "demo.main")], NodeLabel::Function)
            .unwrap();
        storage.delete_file_nodes("/src/main.rs", "demo").expect("delete_file_nodes");
        let file_rows = storage.query("MATCH (f:File) RETURN f.id;").unwrap();
        assert!(file_rows.is_empty());
    }

    // --- delete_file_nodes_batch ---

    #[test]
    fn delete_file_nodes_batch_removes_multiple_files() {
        let repo = fresh_repo();
        repo.save_project(&sample_project("demo", "demo")).unwrap();
        repo.save_nodes(
            &[
                sample_file("f1", "demo", "/src/a.rs", "h1"),
                sample_file("f2", "demo", "/src/b.rs", "h2"),
                sample_file("f3", "demo", "/src/c.rs", "h3"),
            ],
            NodeLabel::File,
        )
        .unwrap();

        repo.delete_file_nodes_batch(&["/src/a.rs".to_string(), "/src/b.rs".to_string()], "demo")
            .expect("delete_file_nodes_batch");

        let rows = repo.connection().query("MATCH (f:File) RETURN f.filePath AS p;").unwrap();
        assert_eq!(rows.len(), 1, "only /src/c.rs should remain");
        assert_eq!(rows[0][0].as_str(), Some("/src/c.rs"));
    }

    #[test]
    fn delete_file_nodes_batch_empty_list_is_noop() {
        let repo = fresh_repo();
        repo.save_project(&sample_project("demo", "demo")).unwrap();
        repo.save_nodes(&[sample_file("f1", "demo", "/src/a.rs", "h1")], NodeLabel::File)
            .unwrap();

        repo.delete_file_nodes_batch(&[], "demo").expect("empty batch noop");

        let rows = repo.connection().query("MATCH (f:File) RETURN f.filePath AS p;").unwrap();
        assert_eq!(rows.len(), 1, "file should still exist");
    }

    #[test]
    fn delete_file_nodes_batch_with_changed_files_removes_them() {
        let repo = fresh_repo();
        repo.save_project(&sample_project("demo", "demo")).unwrap();
        repo.save_nodes(
            &[
                sample_file("f1", "demo", "/src/old.rs", "h1"),
                sample_file("f2", "demo", "/src/new.rs", "h2"),
            ],
            NodeLabel::File,
        )
        .unwrap();

        // Simulate incremental: deleted + changed files.
        let paths = vec!["/src/old.rs".to_string(), "/src/new.rs".to_string()];
        repo.delete_file_nodes_batch(&paths, "demo")
            .expect("batch delete deleted+changed");

        let rows = repo.connection().query("MATCH (f:File) RETURN f.filePath AS p;").unwrap();
        assert!(rows.is_empty(), "both files should be deleted");
    }
}
