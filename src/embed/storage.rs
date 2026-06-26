// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Vector storage in the LadybugDB `Embedding` table (SubTask 16.2).
//!
//! [`EmbeddingStorage`] stores and retrieves `FLOAT[384]` vectors (DDD §5.9).
//! When the LadybugDB VECTOR extension is unavailable, similarity search falls
//! back to in-Rust cosine similarity over all stored vectors for the project.
//!
//! # Table schema (DDD §5.9)
//!
//! ```cypher
//! CREATE NODE TABLE Embedding (
//!     id STRING, nodeId STRING, project STRING, chunkIndex INT32,
//!     startLine INT64, endLine INT64, embedding FLOAT[384],
//!     contentHash STRING, PRIMARY KEY (id)
//! );
//! ```

use std::path::Path;

use crate::storage::StorageConnection;
use uuid::Uuid;

use super::{EmbedError, Result, EMBEDDING_DIM};

/// A single embedding record to store (mirrors the `Embedding` table).
#[derive(Debug, Clone)]
pub struct EmbeddingRecord {
    /// UUIDv7 identifier.
    pub id: String,
    /// Associated code node ID.
    pub node_id: String,
    /// Project name (multi-project isolation).
    pub project: String,
    /// Chunk index (for multi-chunk nodes).
    pub chunk_index: i32,
    /// Start line in source.
    pub start_line: i64,
    /// End line in source.
    pub end_line: i64,
    /// 384-dimensional embedding vector.
    pub embedding: Vec<f32>,
    /// Content hash for deduplication.
    pub content_hash: String,
}

impl EmbeddingRecord {
    /// Creates a new record with a generated UUIDv7.
    #[must_use]
    pub fn new(
        node_id: impl Into<String>,
        project: impl Into<String>,
        start_line: i64,
        end_line: i64,
        embedding: Vec<f32>,
        content_hash: impl Into<String>,
    ) -> Self {
        Self {
            id: format!("emb_{}", Uuid::now_v7().simple()),
            node_id: node_id.into(),
            project: project.into(),
            chunk_index: 0,
            start_line,
            end_line,
            embedding,
            content_hash: content_hash.into(),
        }
    }

    /// Returns `true` if the embedding vector has the expected dimension.
    #[must_use]
    pub fn has_valid_dim(&self) -> bool {
        self.embedding.len() == EMBEDDING_DIM
    }
}

/// A similarity search hit.
#[derive(Debug, Clone)]
pub struct EmbeddingHit {
    /// Node ID of the matched embedding.
    pub node_id: String,
    /// Project name.
    pub project: String,
    /// Cosine similarity score in `[-1.0, 1.0]`.
    pub score: f32,
}

/// Storage for embedding vectors in LadybugDB.
///
/// Wraps a [`StorageConnection`] and provides methods to store, search, and
/// delete embeddings. When the `Embedding` table is unavailable (VECTOR
/// extension missing), operations degrade gracefully.
pub struct EmbeddingStorage<'a> {
    conn: &'a StorageConnection,
}

impl<'a> EmbeddingStorage<'a> {
    /// Creates a storage facade over an existing connection.
    #[must_use]
    pub fn new(conn: &'a StorageConnection) -> Self {
        Self { conn }
    }

    /// Stores a batch of embedding records.
    ///
    /// Each record is inserted via a Cypher `CREATE` statement. If the
    /// `Embedding` table does not exist, returns
    /// [`EmbedError::EmbeddingTableNotAvailable`].
    ///
    /// # Errors
    ///
    /// - [`EmbedError::Storage`] on database failure.
    /// - [`EmbedError::EmbeddingTableNotAvailable`] if the table is missing.
    /// - [`EmbedError::DimensionMismatch`] if any record has wrong dimension.
    pub fn store(&self, records: &[EmbeddingRecord]) -> Result<()> {
        for record in records {
            if !record.has_valid_dim() {
                return Err(EmbedError::DimensionMismatch {
                    expected: EMBEDDING_DIM,
                    actual: record.embedding.len(),
                });
            }
            let cypher = self.build_create_cypher(record);
            match self.conn.execute(&cypher) {
                Ok(()) => {}
                Err(e) => {
                    let msg = e.to_string();
                    if Self::is_table_missing_error(&msg) {
                        return Err(EmbedError::EmbeddingTableNotAvailable);
                    }
                    return Err(EmbedError::Storage(e));
                }
            }
        }
        Ok(())
    }

    /// Searches for embeddings similar to `query_vec` within `project`.
    ///
    /// Retrieves all embeddings for the project and computes cosine similarity
    /// in Rust (fallback when the VECTOR extension is unavailable). Results are
    /// sorted by descending similarity and truncated to `limit`.
    ///
    /// # Errors
    ///
    /// - [`EmbedError::Storage`] on database failure.
    /// - [`EmbedError::EmbeddingTableNotAvailable`] if the table is missing.
    /// - [`EmbedError::DimensionMismatch`] if `query_vec` has wrong dimension.
    pub fn search(
        &self,
        query_vec: &[f32],
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<EmbeddingHit>> {
        if query_vec.len() != EMBEDDING_DIM {
            return Err(EmbedError::DimensionMismatch {
                expected: EMBEDDING_DIM,
                actual: query_vec.len(),
            });
        }

        let filter = match project {
            Some(p) => format!("WHERE e.project = '{}'", Self::escape(p)),
            None => String::new(),
        };
        let cypher = format!(
            "MATCH (e:Embedding) {filter} RETURN e.nodeId AS nodeId, e.project AS project, e.embedding AS embedding;"
        );

        let rows = match self.conn.query(&cypher) {
            Ok(rows) => rows,
            Err(e) => {
                let msg = e.to_string();
                if Self::is_table_missing_error(&msg) {
                    return Err(EmbedError::EmbeddingTableNotAvailable);
                }
                return Err(EmbedError::Storage(e));
            }
        };

        let mut hits: Vec<EmbeddingHit> = rows
            .into_iter()
            .filter_map(|row| {
                let node_id = row.get(0)?.as_str()?.to_string();
                let project = row.get(1)?.as_str()?.to_string();
                let embedding = Self::parse_embedding(row.get(2)?)?;
                let score = cosine_similarity(query_vec, &embedding);
                Some(EmbeddingHit {
                    node_id,
                    project,
                    score,
                })
            })
            .collect();

        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(limit);
        Ok(hits)
    }

    /// Deletes all embeddings for the given project.
    ///
    /// # Errors
    ///
    /// Returns [`EmbedError::Storage`] on database failure. If the table is
    /// missing, the operation is a no-op (returns `Ok(())`).
    pub fn delete_for_project(&self, project: &str) -> Result<()> {
        let cypher = format!(
            "MATCH (e:Embedding {{project: '{}'}}) DELETE e;",
            Self::escape(project)
        );
        match self.conn.execute(&cypher) {
            Ok(()) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                if Self::is_table_missing_error(&msg) {
                    // Table doesn't exist — nothing to delete.
                    Ok(())
                } else {
                    Err(EmbedError::Storage(e))
                }
            }
        }
    }

    /// Returns the count of embeddings for `project` (or all if `None`).
    ///
    /// # Errors
    ///
    /// Returns [`EmbedError::Storage`] on database failure. Returns `Ok(0)` if
    /// the table is missing.
    pub fn count(&self, project: Option<&str>) -> Result<usize> {
        let filter = match project {
            Some(p) => format!("WHERE e.project = '{}'", Self::escape(p)),
            None => String::new(),
        };
        let cypher = format!("MATCH (e:Embedding) {filter} RETURN count(e) AS cnt;");
        match self.conn.query(&cypher) {
            Ok(rows) => {
                let count = rows
                    .first()
                    .and_then(|r| r.first())
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0) as usize;
                Ok(count)
            }
            Err(e) => {
                let msg = e.to_string();
                if Self::is_table_missing_error(&msg) {
                    Ok(0)
                } else {
                    Err(EmbedError::Storage(e))
                }
            }
        }
    }

    /// Builds a Cypher CREATE statement for a single record.
    fn build_create_cypher(&self, record: &EmbeddingRecord) -> String {
        let embedding_list = Self::format_embedding(&record.embedding);
        format!(
            "CREATE (:Embedding {{id: '{}', nodeId: '{}', project: '{}', chunkIndex: {}, \
             startLine: {}, endLine: {}, embedding: {}, contentHash: '{}'}});",
            Self::escape(&record.id),
            Self::escape(&record.node_id),
            Self::escape(&record.project),
            record.chunk_index,
            record.start_line,
            record.end_line,
            embedding_list,
            Self::escape(&record.content_hash),
        )
    }

    /// Formats a vector as a Cypher list literal: `[0.1, 0.2, ...]`.
    fn format_embedding(vec: &[f32]) -> String {
        let items: Vec<String> = vec.iter().map(|f| format!("{f:.6}")).collect();
        format!("[{}]", items.join(", "))
    }

    /// Parses a JSON array value into a `Vec<f32>`.
    fn parse_embedding(value: &serde_json::Value) -> Option<Vec<f32>> {
        value.as_array().map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect()
        })
    }

    /// Escapes single quotes in a string for safe Cypher interpolation.
    fn escape(s: &str) -> String {
        s.replace('\'', "\\'")
    }

    /// Returns `true` if the error message indicates the Embedding table is missing.
    fn is_table_missing_error(msg: &str) -> bool {
        let lower = msg.to_ascii_lowercase();
        lower.contains("embedding") && (lower.contains("not exist") || lower.contains("not found"))
    }
}

/// Computes cosine similarity between two vectors.
///
/// Returns `0.0` if either vector has zero magnitude.
#[must_use]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let mag_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }
    dot / (mag_a * mag_b)
}

/// Opens a storage connection at `db_path` (convenience for CLI integration).
///
/// # Errors
///
/// Returns [`EmbedError::Storage`] if the database cannot be opened.
pub fn open_storage(db_path: &Path) -> Result<StorageConnection> {
    let conn = StorageConnection::open(db_path)?;
    conn.init_schema()?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_conn() -> StorageConnection {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = StorageConnection::open(dir.path().join("embed_testdb")).expect("open");
        conn.init_schema().expect("init_schema");
        std::mem::forget(dir);
        conn
    }

    fn make_embedding(seed: f32) -> Vec<f32> {
        (0..EMBEDDING_DIM)
            .map(|i| seed + i as f32 * 0.001)
            .collect()
    }

    fn make_record(id: &str, node_id: &str, project: &str, embedding: Vec<f32>) -> EmbeddingRecord {
        EmbeddingRecord {
            id: id.to_string(),
            node_id: node_id.to_string(),
            project: project.to_string(),
            chunk_index: 0,
            start_line: 1,
            end_line: 10,
            embedding,
            content_hash: "abc123".to_string(),
        }
    }

    // --- EmbeddingRecord ---

    #[test]
    fn record_new_generates_id() {
        let emb = make_embedding(0.1);
        let rec = EmbeddingRecord::new("node1", "demo", 1, 10, emb, "hash123");
        assert!(
            rec.id.starts_with("emb_"),
            "id should start with emb_: {}",
            rec.id
        );
        assert_eq!(rec.node_id, "node1");
        assert_eq!(rec.project, "demo");
        assert_eq!(rec.chunk_index, 0);
        assert_eq!(rec.content_hash, "hash123");
    }

    #[test]
    fn record_has_valid_dim() {
        let rec = make_record("e1", "n1", "p", make_embedding(0.1));
        assert!(rec.has_valid_dim());

        let rec_bad = EmbeddingRecord {
            id: "e2".into(),
            node_id: "n2".into(),
            project: "p".into(),
            chunk_index: 0,
            start_line: 1,
            end_line: 2,
            embedding: vec![0.1; 128],
            content_hash: "h".into(),
        };
        assert!(!rec_bad.has_valid_dim());
    }

    // --- cosine_similarity ---

    #[test]
    fn cosine_similarity_identical_vectors() {
        let v = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v);
        assert!(
            (sim - 1.0).abs() < 1e-5,
            "identical vectors should have sim=1.0, got {sim}"
        );
    }

    #[test]
    fn cosine_similarity_orthogonal_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!(
            sim.abs() < 1e-5,
            "orthogonal vectors should have sim=0.0, got {sim}"
        );
    }

    #[test]
    fn cosine_similarity_zero_vector() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert!(
            sim.abs() < 1e-5,
            "zero vector should have sim=0.0, got {sim}"
        );
    }

    #[test]
    fn cosine_similarity_different_lengths() {
        let a = vec![1.0, 2.0];
        let b = vec![1.0];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0, "different lengths should return 0.0");
    }

    #[test]
    fn cosine_similarity_empty_vectors() {
        let sim = cosine_similarity(&[], &[]);
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn cosine_similarity_opposite_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(
            (sim + 1.0).abs() < 1e-5,
            "opposite vectors should have sim=-1.0, got {sim}"
        );
    }

    // --- EmbeddingStorage::store ---

    #[test]
    fn store_inserts_record() {
        let conn = fresh_conn();
        let storage = EmbeddingStorage::new(&conn);
        let rec = make_record("e1", "n1", "demo", make_embedding(0.1));
        let result = storage.store(&[rec]);
        // If the Embedding table is not available, skip this test.
        match result {
            Ok(()) => {
                let count = storage.count(Some("demo")).expect("count");
                assert_eq!(count, 1, "should have 1 embedding");
            }
            Err(EmbedError::EmbeddingTableNotAvailable) => {
                // VECTOR extension not available — skip.
            }
            Err(e) => panic!("unexpected error: {e}"),
        }
    }

    #[test]
    fn store_multiple_records() {
        let conn = fresh_conn();
        let storage = EmbeddingStorage::new(&conn);
        let recs = vec![
            make_record("e1", "n1", "demo", make_embedding(0.1)),
            make_record("e2", "n2", "demo", make_embedding(0.2)),
            make_record("e3", "n3", "demo", make_embedding(0.3)),
        ];
        match storage.store(&recs) {
            Ok(()) => {
                let count = storage.count(Some("demo")).expect("count");
                assert_eq!(count, 3);
            }
            Err(EmbedError::EmbeddingTableNotAvailable) => {}
            Err(e) => panic!("unexpected: {e}"),
        }
    }

    #[test]
    fn store_rejects_wrong_dimension() {
        let conn = fresh_conn();
        let storage = EmbeddingStorage::new(&conn);
        let bad_rec = EmbeddingRecord {
            id: "e1".into(),
            node_id: "n1".into(),
            project: "demo".into(),
            chunk_index: 0,
            start_line: 1,
            end_line: 2,
            embedding: vec![0.1; 128],
            content_hash: "h".into(),
        };
        let result = storage.store(&[bad_rec]);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            EmbedError::DimensionMismatch {
                expected: 384,
                actual: 128
            }
        ));
    }

    #[test]
    fn store_empty_batch_is_noop() {
        let conn = fresh_conn();
        let storage = EmbeddingStorage::new(&conn);
        let result = storage.store(&[]);
        assert!(result.is_ok(), "empty batch should succeed");
    }

    // --- EmbeddingStorage::search ---

    #[test]
    fn search_returns_similar_embeddings() {
        let conn = fresh_conn();
        let storage = EmbeddingStorage::new(&conn);
        let emb1 = make_embedding(0.1);
        let emb2 = make_embedding(0.5);
        let recs = vec![
            make_record("e1", "n1", "demo", emb1.clone()),
            make_record("e2", "n2", "demo", emb2.clone()),
        ];
        match storage.store(&recs) {
            Ok(()) => {
                let hits = storage.search(&emb1, Some("demo"), 10).expect("search");
                assert_eq!(hits.len(), 2, "should find 2 hits");
                // Most similar should be emb1 itself (sim=1.0).
                assert!(
                    (hits[0].score - 1.0).abs() < 1e-3,
                    "top hit should be identical, got score={}",
                    hits[0].score
                );
                assert_eq!(hits[0].node_id, "n1");
            }
            Err(EmbedError::EmbeddingTableNotAvailable) => {}
            Err(e) => panic!("unexpected: {e}"),
        }
    }

    #[test]
    fn search_respects_limit() {
        let conn = fresh_conn();
        let storage = EmbeddingStorage::new(&conn);
        let recs: Vec<_> = (0..5)
            .map(|i| {
                make_record(
                    &format!("e{i}"),
                    &format!("n{i}"),
                    "demo",
                    make_embedding(i as f32 * 0.1),
                )
            })
            .collect();
        match storage.store(&recs) {
            Ok(()) => {
                let hits = storage
                    .search(&make_embedding(0.0), Some("demo"), 2)
                    .expect("search");
                assert!(hits.len() <= 2, "should respect limit");
            }
            Err(EmbedError::EmbeddingTableNotAvailable) => {}
            Err(e) => panic!("unexpected: {e}"),
        }
    }

    #[test]
    fn search_filters_by_project() {
        let conn = fresh_conn();
        let storage = EmbeddingStorage::new(&conn);
        let recs = vec![
            make_record("e1", "n1", "alpha", make_embedding(0.1)),
            make_record("e2", "n2", "beta", make_embedding(0.1)),
        ];
        match storage.store(&recs) {
            Ok(()) => {
                let hits = storage
                    .search(&make_embedding(0.1), Some("alpha"), 10)
                    .expect("search");
                assert!(
                    hits.iter().all(|h| h.project == "alpha"),
                    "should only return alpha"
                );
            }
            Err(EmbedError::EmbeddingTableNotAvailable) => {}
            Err(e) => panic!("unexpected: {e}"),
        }
    }

    #[test]
    fn search_wrong_query_dim_returns_error() {
        let conn = fresh_conn();
        let storage = EmbeddingStorage::new(&conn);
        let result = storage.search(&[0.1; 128], None, 10);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            EmbedError::DimensionMismatch { .. }
        ));
    }

    #[test]
    fn search_empty_db_returns_empty() {
        let conn = fresh_conn();
        let storage = EmbeddingStorage::new(&conn);
        match storage.search(&make_embedding(0.1), Some("demo"), 10) {
            Ok(hits) => assert!(hits.is_empty(), "empty db should return no hits"),
            Err(EmbedError::EmbeddingTableNotAvailable) => {}
            Err(e) => panic!("unexpected: {e}"),
        }
    }

    // --- EmbeddingStorage::delete_for_project ---

    #[test]
    fn delete_removes_embeddings() {
        let conn = fresh_conn();
        let storage = EmbeddingStorage::new(&conn);
        let recs = vec![
            make_record("e1", "n1", "demo", make_embedding(0.1)),
            make_record("e2", "n2", "demo", make_embedding(0.2)),
        ];
        match storage.store(&recs) {
            Ok(()) => {
                storage.delete_for_project("demo").expect("delete");
                let count = storage.count(Some("demo")).expect("count");
                assert_eq!(count, 0, "should have 0 after delete");
            }
            Err(EmbedError::EmbeddingTableNotAvailable) => {}
            Err(e) => panic!("unexpected: {e}"),
        }
    }

    #[test]
    fn delete_missing_project_is_noop() {
        let conn = fresh_conn();
        let storage = EmbeddingStorage::new(&conn);
        let result = storage.delete_for_project("nonexistent");
        assert!(
            result.is_ok(),
            "deleting nonexistent project should succeed"
        );
    }

    // --- EmbeddingStorage::count ---

    #[test]
    fn count_returns_zero_on_empty() {
        let conn = fresh_conn();
        let storage = EmbeddingStorage::new(&conn);
        match storage.count(Some("demo")) {
            Ok(0) => {}
            Ok(n) => panic!("expected 0, got {n}"),
            Err(EmbedError::EmbeddingTableNotAvailable) => {}
            Err(e) => panic!("unexpected: {e}"),
        }
    }

    #[test]
    fn count_all_projects() {
        let conn = fresh_conn();
        let storage = EmbeddingStorage::new(&conn);
        let recs = vec![
            make_record("e1", "n1", "alpha", make_embedding(0.1)),
            make_record("e2", "n2", "beta", make_embedding(0.2)),
        ];
        match storage.store(&recs) {
            Ok(()) => {
                let count = storage.count(None).expect("count");
                assert_eq!(count, 2, "should count all projects");
            }
            Err(EmbedError::EmbeddingTableNotAvailable) => {}
            Err(e) => panic!("unexpected: {e}"),
        }
    }

    // --- Helper function tests ---

    #[test]
    fn format_embedding_produces_list() {
        let s = EmbeddingStorage::format_embedding(&[0.1, 0.2, 0.3]);
        assert!(s.starts_with('['), "should start with [: {s}");
        assert!(s.ends_with(']'), "should end with ]: {s}");
        assert!(
            s.contains("0.100000"),
            "should contain formatted float: {s}"
        );
    }

    #[test]
    fn parse_embedding_from_json_array() {
        let json = serde_json::json!([0.1, 0.2, 0.3]);
        let result = EmbeddingStorage::parse_embedding(&json);
        assert!(result.is_some());
        let vec = result.unwrap();
        assert_eq!(vec.len(), 3);
        assert!((vec[0] - 0.1).abs() < 1e-5);
    }

    #[test]
    fn parse_embedding_non_array_returns_none() {
        let json = serde_json::json!("not an array");
        assert!(EmbeddingStorage::parse_embedding(&json).is_none());
    }

    #[test]
    fn escape_single_quotes() {
        assert_eq!(EmbeddingStorage::escape("it's"), "it\\'s");
        assert_eq!(EmbeddingStorage::escape("normal"), "normal");
    }

    #[test]
    fn is_table_missing_error_detects_patterns() {
        assert!(EmbeddingStorage::is_table_missing_error(
            "Table Embedding does not exist"
        ));
        assert!(EmbeddingStorage::is_table_missing_error(
            "embedding not found"
        ));
        assert!(!EmbeddingStorage::is_table_missing_error(
            "syntax error near CREATE"
        ));
    }

    // --- open_storage ---

    #[test]
    fn open_storage_creates_connection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("open_storage_testdb");
        let result = open_storage(&path);
        assert!(result.is_ok(), "should open storage: {:?}", result.err());
        std::mem::forget(dir);
    }

    #[test]
    fn open_storage_nonexistent_dir_errors() {
        let result = open_storage(std::path::Path::new("/nonexistent/dir/db"));
        assert!(result.is_err(), "should error for nonexistent dir");
    }
}
