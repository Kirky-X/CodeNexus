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

/// Executes raw Cypher queries against a [`StorageConnection`].
///
/// Borrows the connection for its lifetime; obtain one via
/// [`CypherExecutor::new`].
pub struct CypherExecutor<'a> {
    conn: &'a StorageConnection,
}

impl<'a> CypherExecutor<'a> {
    /// Creates a new [`CypherExecutor`] borrowing `conn`.
    #[must_use]
    pub fn new(conn: &'a StorageConnection) -> Self {
        Self { conn }
    }

    /// Executes a Cypher query and returns the columns, rows, and duration.
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
        let start = Instant::now();
        let (columns, rows) = self.conn.query_with_columns(cypher)?;
        let duration_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
        Ok(QueryResult {
            columns,
            rows,
            duration_ms,
        })
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
