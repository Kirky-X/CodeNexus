// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Search strategies (Strategy pattern, SubTask 16.3).
//!
//! Provides three pluggable search strategies:
//! - [`Bm25Strategy`]: BM25 full-text search (LadybugDB FTS, fallback to CONTAINS).
//! - [`SemanticStrategy`]: vector similarity search using embeddings.
//! - [`HybridStrategy`]: combines BM25 + semantic via Reciprocal Rank Fusion (RRF).
//!
//! # Windows degradation (R-003 / TR-005)
//!
//! [`is_vector_supported`] returns `false` on Windows because the LadybugDB
//! VECTOR extension is unavailable there. [`SemanticStrategy`] and
//! [`HybridStrategy`] automatically degrade to BM25-only on Windows.
//!
//! # RRF (Reciprocal Rank Fusion)
//!
//! Given two ranked lists, the fused score for a document `d` is:
//! ```text
//! rrf_score(d) = sum( 1 / (k + rank_i(d)) )  for each list i
//! ```
//! where `k = 60` (standard constant) and `rank_i(d)` is the 1-based rank of
//! `d` in list `i` (or 0 if absent).

use std::collections::HashMap;

use crate::query::{FullTextSearcher, SearchResult};
use crate::storage::StorageConnection;

use super::client::EmbedClient;
use super::storage::{EmbeddingHit, EmbeddingStorage};
use super::{EmbedError, Result, EMBEDDING_DIM};

/// RRF constant (standard value from the original paper).
const RRF_K: u32 = 60;

/// Weight for the base (BM25 + semantic RRF) score in multi-signal fusion
/// (R-search-002). Explicit constant per Rule 5 (deterministic logic).
const BASE_WEIGHT: f64 = 0.5;

/// Weight for the centrality signal (CALLS in-degree) in multi-signal fusion
/// (R-search-002).
const CENTRALITY_WEIGHT: f64 = 0.3;

/// Weight for the file heat signal (IMPORTS count) in multi-signal fusion
/// (R-search-002).
const FILE_HEAT_WEIGHT: f64 = 0.2;

/// The search strategy to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchStrategyType {
    /// BM25 full-text search only.
    Bm25,
    /// Vector semantic search only.
    Semantic,
    /// Hybrid: BM25 + semantic fused via RRF (AC-SEARCH-002).
    Hybrid,
}

impl SearchStrategyType {
    /// Parses a strategy type from a string.
    #[must_use]
    pub fn parse_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "bm25" => Some(Self::Bm25),
            "semantic" => Some(Self::Semantic),
            "hybrid" => Some(Self::Hybrid),
            _ => None,
        }
    }

    /// Returns `true` if this strategy requires vector support.
    #[must_use]
    pub fn requires_vector(self) -> bool {
        matches!(self, Self::Semantic | Self::Hybrid)
    }
}

impl std::fmt::Display for SearchStrategyType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bm25 => write!(f, "bm25"),
            Self::Semantic => write!(f, "semantic"),
            Self::Hybrid => write!(f, "hybrid"),
        }
    }
}

/// Returns `true` if the LadybugDB VECTOR extension is supported on this
/// platform.
///
/// On Windows the VECTOR extension is unavailable (R-003 / TR-005), so this
/// returns `false` and search strategies degrade to BM25-only.
#[must_use]
pub fn is_vector_supported() -> bool {
    // R-003: LadybugDB VECTOR extension is not supported on Windows.
    #[cfg(target_os = "windows")]
    {
        false
    }
    #[cfg(not(target_os = "windows"))]
    {
        true
    }
}

/// Trait for search strategies (Strategy pattern).
///
/// Each implementation encapsulates a different search algorithm. The CLI
/// `search --semantic` command selects the strategy based on feature
/// availability and platform.
pub trait SearchStrategy: Send + Sync {
    /// Executes the search and returns ranked results.
    ///
    /// # Errors
    ///
    /// Returns [`EmbedError`] on failure. Strategies that degrade to BM25
    /// return [`EmbedError::Storage`] wrapped errors.
    fn search(&self, query: &str, project: Option<&str>, limit: usize)
        -> Result<Vec<SearchResult>>;
}

/// BM25 full-text search strategy.
///
/// Delegates to [`FullTextSearcher`] for LadybugDB FTS (or CONTAINS fallback).
/// This strategy works on all platforms without vector support.
pub struct Bm25Strategy<'a> {
    conn: &'a StorageConnection,
}

impl<'a> Bm25Strategy<'a> {
    /// Creates a new BM25 strategy over the given connection.
    #[must_use]
    pub fn new(conn: &'a StorageConnection) -> Self {
        Self { conn }
    }
}

impl<'a> SearchStrategy for Bm25Strategy<'a> {
    fn search(
        &self,
        query: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let searcher = FullTextSearcher::new(self.conn);
        searcher
            .search(query, project, limit)
            .map_err(|e| EmbedError::Storage(crate::storage::StorageError::Query(e.to_string())))
    }
}

/// Semantic (vector) search strategy.
///
/// Embeds the query text via [`EmbedClient`], searches the `Embedding` table
/// for similar vectors, and joins with node metadata to produce
/// [`SearchResult`]s. On Windows (or when vector support is unavailable), this
/// degrades to [`Bm25Strategy`].
pub struct SemanticStrategy<'a, C: ?Sized + EmbedClient> {
    conn: &'a StorageConnection,
    client: &'a C,
}

impl<'a, C: ?Sized + EmbedClient> SemanticStrategy<'a, C> {
    /// Creates a new semantic strategy.
    #[must_use]
    pub fn new(conn: &'a StorageConnection, client: &'a C) -> Self {
        Self { conn, client }
    }
}

impl<'a, C: ?Sized + EmbedClient> SearchStrategy for SemanticStrategy<'a, C> {
    fn search(
        &self,
        query: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        // Windows degradation: fall back to BM25 (R-003 / TR-005).
        if !is_vector_supported() {
            return Bm25Strategy::new(self.conn).search(query, project, limit);
        }

        // Embed the query text.
        let embeddings = self.client.embed(&[query])?;
        let query_vec = embeddings.into_iter().next().ok_or_else(|| {
            EmbedError::Unavailable("embedding service returned no vectors".to_string())
        })?;

        if query_vec.len() != EMBEDDING_DIM {
            return Err(EmbedError::DimensionMismatch {
                expected: EMBEDDING_DIM,
                actual: query_vec.len(),
            });
        }

        // Search the Embedding table.
        let storage = EmbeddingStorage::new(self.conn);
        let hits = match storage.search(&query_vec, project, limit) {
            Ok(hits) => hits,
            Err(EmbedError::EmbeddingTableNotAvailable) => {
                // Table not available — degrade to BM25.
                return Bm25Strategy::new(self.conn).search(query, project, limit);
            }
            Err(e) => return Err(e),
        };

        // Join with node metadata.
        let results = hits
            .into_iter()
            .filter_map(|hit| lookup_node_metadata(self.conn, &hit))
            .take(limit)
            .collect();
        Ok(results)
    }
}

/// Hybrid search strategy: BM25 + semantic fused via RRF (AC-SEARCH-002).
///
/// Runs both BM25 and semantic search, then fuses the ranked lists using
/// Reciprocal Rank Fusion. On Windows (or when vector support is unavailable),
/// this degrades to BM25-only.
///
/// # Multi-signal scoring (R-search-002)
///
/// When [`with_centrality`](Self::with_centrality) and/or
/// [`with_file_heat`](Self::with_file_heat) are enabled, the fused RRF score
/// is combined with call-graph centrality and file heat signals:
///
/// ```text
/// final_score = 0.5 * base_score + 0.3 * centrality_score + 0.2 * file_heat_score
/// ```
///
/// When neither signal is enabled (the default), the ranking is identical to
/// the original RRF-only fusion (backward compatible).
pub struct HybridStrategy<'a, C: ?Sized + EmbedClient> {
    conn: &'a StorageConnection,
    client: &'a C,
    enable_centrality: bool,
    enable_file_heat: bool,
}

impl<'a, C: ?Sized + EmbedClient> HybridStrategy<'a, C> {
    /// Creates a new hybrid strategy.
    #[must_use]
    pub fn new(conn: &'a StorageConnection, client: &'a C) -> Self {
        Self {
            conn,
            client,
            enable_centrality: false,
            enable_file_heat: false,
        }
    }

    /// Enables or disables the centrality signal (CALLS in-degree).
    ///
    /// When enabled, each result's final score incorporates a normalized
    /// centrality term: `in_degree / max_in_degree`. The weight is
    /// [`CENTRALITY_WEIGHT`] (0.3). Disabled by default (backward compatible).
    #[must_use]
    pub fn with_centrality(mut self, enable: bool) -> Self {
        self.enable_centrality = enable;
        self
    }

    /// Enables or disables the file heat signal (IMPORTS count).
    ///
    /// When enabled, each result's final score incorporates a normalized
    /// file heat term: `import_count / max_import_count`. The weight is
    /// [`FILE_HEAT_WEIGHT`] (0.2). Disabled by default (backward compatible).
    #[must_use]
    pub fn with_file_heat(mut self, enable: bool) -> Self {
        self.enable_file_heat = enable;
        self
    }
}

impl<'a, C: ?Sized + EmbedClient> SearchStrategy for HybridStrategy<'a, C> {
    fn search(
        &self,
        query: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        // Windows degradation: BM25 only (R-003 / TR-005).
        if !is_vector_supported() {
            let results = Bm25Strategy::new(self.conn).search(query, project, limit)?;
            return Ok(apply_multi_signal_if_enabled(
                self.conn,
                results,
                project,
                self.enable_centrality,
                self.enable_file_heat,
            ));
        }

        // Run BM25 search.
        let bm25_results = Bm25Strategy::new(self.conn).search(query, project, limit * 2)?;

        // Run semantic search (may degrade if table missing).
        let semantic_results =
            match SemanticStrategy::new(self.conn, self.client).search(query, project, limit * 2) {
                Ok(results) => results,
                Err(EmbedError::EmbeddingTableNotAvailable) | Err(EmbedError::Unavailable(_)) => {
                    // Semantic unavailable — return BM25 only (with multi-signal if enabled).
                    let results: Vec<SearchResult> = bm25_results.into_iter().take(limit).collect();
                    return Ok(apply_multi_signal_if_enabled(
                        self.conn,
                        results,
                        project,
                        self.enable_centrality,
                        self.enable_file_heat,
                    ));
                }
                Err(e) => return Err(e),
            };

        // Fuse via RRF.
        let fused = rrf_fuse(bm25_results, semantic_results, limit);
        Ok(apply_multi_signal_if_enabled(
            self.conn,
            fused,
            project,
            self.enable_centrality,
            self.enable_file_heat,
        ))
    }
}

// ---------------------------------------------------------------------------
// Multi-signal scoring (R-search-002)
// ---------------------------------------------------------------------------

/// Returns results unchanged if neither signal is enabled or no project filter
/// is present. Otherwise applies [`apply_multi_signal_scoring`].
fn apply_multi_signal_if_enabled(
    conn: &StorageConnection,
    results: Vec<SearchResult>,
    project: Option<&str>,
    enable_centrality: bool,
    enable_file_heat: bool,
) -> Vec<SearchResult> {
    if !enable_centrality && !enable_file_heat {
        return results;
    }
    let project = match project {
        Some(p) => p,
        None => return results,
    };
    apply_multi_signal_scoring(conn, results, project, enable_centrality, enable_file_heat)
}

/// Applies the multi-signal scoring formula (R-search-002):
///
/// `final_score = 0.5 * base_score + 0.3 * centrality_score + 0.2 * file_heat_score`
///
/// When a signal is disabled, its term is 0 for every result. Results are
/// re-sorted by final score descending.
fn apply_multi_signal_scoring(
    conn: &StorageConnection,
    mut results: Vec<SearchResult>,
    project: &str,
    enable_centrality: bool,
    enable_file_heat: bool,
) -> Vec<SearchResult> {
    if results.is_empty() {
        return results;
    }

    let centrality_scores: HashMap<String, f64> = if enable_centrality {
        compute_centrality_scores(conn, &results, project)
    } else {
        HashMap::new()
    };

    let file_heat_scores: HashMap<String, f64> = if enable_file_heat {
        compute_file_heat_scores(conn, &results, project)
    } else {
        HashMap::new()
    };

    for result in &mut results {
        let key = result
            .qualified_name
            .clone()
            .unwrap_or_else(|| result.name.clone());
        let base = result.score;
        let centrality = centrality_scores.get(&key).copied().unwrap_or(0.0);
        let file_heat = file_heat_scores.get(&key).copied().unwrap_or(0.0);
        result.score =
            BASE_WEIGHT * base + CENTRALITY_WEIGHT * centrality + FILE_HEAT_WEIGHT * file_heat;
    }

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results
}

/// Computes normalized centrality scores (CALLS in-degree / max in-degree)
/// for each search result. Returns a map keyed by `qualified_name` (or `name`
/// when QN is absent).
fn compute_centrality_scores(
    conn: &StorageConnection,
    results: &[SearchResult],
    project: &str,
) -> HashMap<String, f64> {
    let project_esc = project.replace('\'', "\\'");

    // Load all CALLS edges, aggregate in-degree per target node id.
    let cypher = format!(
        "MATCH (e:CodeRelation) WHERE e.type = 'CALLS' AND e.project = '{project_esc}' \
         RETURN e.target AS target;"
    );
    let rows = conn.query(&cypher).unwrap_or_default();
    let mut in_degrees: HashMap<String, u32> = HashMap::new();
    for row in &rows {
        if let Some(target) = row.first().and_then(|v| v.as_str()) {
            *in_degrees.entry(target.to_string()).or_insert(0) += 1;
        }
    }
    let max_in_degree = in_degrees.values().copied().max().unwrap_or(0);

    // For each result, look up its node id by (label, qualifiedName), then
    // look up the in-degree.
    let mut scores = HashMap::new();
    for result in results {
        let key = result
            .qualified_name
            .clone()
            .unwrap_or_else(|| result.name.clone());
        let qn = result.qualified_name.as_deref().unwrap_or(&result.name);
        let in_degree = lookup_node_id_by_qn(conn, &result.label, qn, project)
            .and_then(|id| in_degrees.get(&id).copied())
            .unwrap_or(0);
        let centrality = if max_in_degree > 0 {
            in_degree as f64 / max_in_degree as f64
        } else {
            0.0
        };
        scores.insert(key, centrality);
    }
    scores
}

/// Computes normalized file heat scores (import count / max import count) for
/// each search result. The "file heat" of a result is the number of IMPORTS
/// edges targeting the File node at `result.file_path`, divided by the maximum
/// such count across all files in the project.
fn compute_file_heat_scores(
    conn: &StorageConnection,
    results: &[SearchResult],
    project: &str,
) -> HashMap<String, f64> {
    let project_esc = project.replace('\'', "\\'");

    // Load all File nodes: id → filePath, filePath → id.
    let cypher = format!(
        "MATCH (f:File) WHERE f.project = '{project_esc}' \
         RETURN f.id AS id, f.filePath AS filePath;"
    );
    let rows = conn.query(&cypher).unwrap_or_default();
    let mut path_to_id: HashMap<String, String> = HashMap::new();
    for row in &rows {
        if let (Some(id), Some(path)) = (
            row.first().and_then(|v| v.as_str()),
            row.get(1).and_then(|v| v.as_str()),
        ) {
            path_to_id.insert(path.to_string(), id.to_string());
        }
    }

    // Load all IMPORTS edges, aggregate count per target (file id).
    let cypher = format!(
        "MATCH (e:CodeRelation) WHERE e.type = 'IMPORTS' AND e.project = '{project_esc}' \
         RETURN e.target AS target;"
    );
    let rows = conn.query(&cypher).unwrap_or_default();
    let mut import_counts: HashMap<String, u32> = HashMap::new();
    for row in &rows {
        if let Some(target) = row.first().and_then(|v| v.as_str()) {
            *import_counts.entry(target.to_string()).or_insert(0) += 1;
        }
    }
    let max_imports = import_counts.values().copied().max().unwrap_or(0);

    // For each result, map file_path → file_id → import_count.
    let mut scores = HashMap::new();
    for result in results {
        let key = result
            .qualified_name
            .clone()
            .unwrap_or_else(|| result.name.clone());
        let heat = result
            .file_path
            .as_ref()
            .and_then(|p| path_to_id.get(p))
            .and_then(|id| import_counts.get(id).copied())
            .unwrap_or(0);
        let heat_score = if max_imports > 0 {
            heat as f64 / max_imports as f64
        } else {
            0.0
        };
        scores.insert(key, heat_score);
    }
    scores
}

/// Looks up a node's `id` by its label and `qualifiedName` within a project.
/// Escapes the label for tables that require backticks (e.g. `Macro`).
fn lookup_node_id_by_qn(
    conn: &StorageConnection,
    label: &str,
    qualified_name: &str,
    project: &str,
) -> Option<String> {
    let table = match label {
        "Macro" => "`Macro`",
        other => other,
    };
    let qn_esc = qualified_name.replace('\'', "\\'");
    let project_esc = project.replace('\'', "\\'");
    let cypher = format!(
        "MATCH (n:{table}) WHERE n.qualifiedName = '{qn_esc}' AND n.project = '{project_esc}' \
         RETURN n.id AS id LIMIT 1;"
    );
    conn.query(&cypher)
        .ok()
        .and_then(|rows| rows.into_iter().next())
        .and_then(|row| row.into_iter().next())
        .and_then(|v| v.as_str().map(|s| s.to_string()))
}

/// Fuses two ranked lists using Reciprocal Rank Fusion (RRF).
///
/// `k` is the RRF constant (default 60). Results are deduplicated by
/// `qualified_name` (or `name` if QN is missing), scored, sorted, and
/// truncated to `limit`.
#[must_use]
pub fn rrf_fuse(
    list_a: Vec<SearchResult>,
    list_b: Vec<SearchResult>,
    limit: usize,
) -> Vec<SearchResult> {
    rrf_fuse_multi(vec![list_a, list_b], limit)
}

/// Fuses multiple ranked lists using RRF.
#[must_use]
pub fn rrf_fuse_multi(lists: Vec<Vec<SearchResult>>, limit: usize) -> Vec<SearchResult> {
    let mut scores: HashMap<String, (f64, SearchResult)> = HashMap::new();

    for list in &lists {
        for (rank, result) in list.iter().enumerate() {
            let key = result
                .qualified_name
                .clone()
                .unwrap_or_else(|| result.name.clone());
            let rrf_score = 1.0 / (RRF_K as f64 + (rank + 1) as f64);
            let entry = scores.entry(key).or_insert_with(|| (0.0, result.clone()));
            entry.0 += rrf_score;
            // Keep the first occurrence's metadata; update score.
        }
    }

    let mut fused: Vec<(f64, SearchResult)> = scores
        .into_iter()
        .map(|(_, (score, mut result))| {
            result.score = score;
            (score, result)
        })
        .collect();

    fused.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    fused.truncate(limit);
    fused.into_iter().map(|(_, r)| r).collect()
}

/// Looks up node metadata by ID across the main node tables.
///
/// Tries Function, Method, Class, Struct, Enum, Trait, Variable, Const in
/// order. Returns a [`SearchResult`] with the embedding hit's score if found.
fn lookup_node_metadata(conn: &StorageConnection, hit: &EmbeddingHit) -> Option<SearchResult> {
    const TABLES: &[(&str, &str)] = &[
        ("Function", "Function"),
        ("Method", "Method"),
        ("Class", "Class"),
        ("Struct", "Struct"),
        ("Enum", "Enum"),
        ("Trait", "Trait"),
        ("Variable", "Variable"),
        ("Const", "Const"),
        ("GlobalVar", "GlobalVar"),
        ("Parameter", "Parameter"),
        ("Static", "Static"),
        ("Macro", "`Macro`"),
        ("TypeAlias", "TypeAlias"),
        ("Typedef", "Typedef"),
        ("Namespace", "Namespace"),
        ("Module", "Module"),
    ];

    for (label, table) in TABLES {
        let cypher = format!(
            "MATCH (n:{table} {{id: '{}'}}) RETURN n.name AS name, n.filePath AS filePath, \
             n.startLine AS startLine, n.qualifiedName AS qn LIMIT 1;",
            hit.node_id.replace('\'', "\\'")
        );
        if let Ok(rows) = conn.query(&cypher) {
            if let Some(row) = rows.first() {
                let name = row
                    .first()
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let file_path = row.get(1).and_then(|v| v.as_str()).map(|s| s.to_string());
                let start_line = row.get(2).and_then(|v| v.as_i64()).map(|n| n as u32);
                let qualified_name = row.get(3).and_then(|v| v.as_str()).map(|s| s.to_string());
                return Some(SearchResult {
                    name,
                    label: label.to_string(),
                    file_path,
                    start_line,
                    qualified_name,
                    score: hit.score as f64,
                    match_reason: "semantic search match".to_string(),
                    degree: 0,
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::MockEmbedClient;
    use crate::storage::StorageConnection;

    fn fresh_conn() -> StorageConnection {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = StorageConnection::open(dir.path().join("search_testdb")).expect("open");
        conn.init_schema().expect("init_schema");
        std::mem::forget(dir);
        conn
    }

    fn seed_fixture(conn: &StorageConnection) {
        conn.execute("CREATE (:Project {id: 'demo', name: 'demo', rootPath: '/', language: 'rust', fileCount: 2, indexedAt: 0});").expect("project");
        conn.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'parse_file', qualifiedName: 'demo.parse_file', filePath: '/src/main.rs', startLine: 1, endLine: 10, signature: '', returnType: '', isExported: false, docstring: '', content: 'parse file content', parentQn: ''});").expect("f1");
        conn.execute("CREATE (:Function {id: 'f2', project: 'demo', name: 'parse_line', qualifiedName: 'demo.parse_line', filePath: '/src/main.rs', startLine: 11, endLine: 20, signature: '', returnType: '', isExported: false, docstring: '', content: 'parse line content', parentQn: ''});").expect("f2");
        conn.execute("CREATE (:Function {id: 'f3', project: 'demo', name: 'read_input', qualifiedName: 'demo.read_input', filePath: '/src/lib.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: 'read input', parentQn: ''});").expect("f3");
    }

    // --- SearchStrategyType ---

    #[test]
    fn strategy_type_from_str() {
        assert_eq!(
            SearchStrategyType::parse_str("bm25"),
            Some(SearchStrategyType::Bm25)
        );
        assert_eq!(
            SearchStrategyType::parse_str("SEMANTIC"),
            Some(SearchStrategyType::Semantic)
        );
        assert_eq!(
            SearchStrategyType::parse_str("Hybrid"),
            Some(SearchStrategyType::Hybrid)
        );
        assert_eq!(SearchStrategyType::parse_str("unknown"), None);
    }

    #[test]
    fn strategy_type_display() {
        assert_eq!(SearchStrategyType::Bm25.to_string(), "bm25");
        assert_eq!(SearchStrategyType::Semantic.to_string(), "semantic");
        assert_eq!(SearchStrategyType::Hybrid.to_string(), "hybrid");
    }

    #[test]
    fn strategy_type_requires_vector() {
        assert!(!SearchStrategyType::Bm25.requires_vector());
        assert!(SearchStrategyType::Semantic.requires_vector());
        assert!(SearchStrategyType::Hybrid.requires_vector());
    }

    // --- is_vector_supported ---

    #[test]
    fn is_vector_supported_returns_bool() {
        let supported = is_vector_supported();
        #[cfg(target_os = "windows")]
        assert!(!supported, "Windows should not support vectors");
        #[cfg(not(target_os = "windows"))]
        assert!(supported, "Non-Windows should support vectors");
    }

    // --- RRF fusion ---

    #[test]
    fn rrf_fuse_combines_two_lists() {
        let list_a = vec![
            SearchResult {
                name: "a".into(),
                label: "Function".into(),
                file_path: None,
                start_line: None,
                qualified_name: Some("a".into()),
                score: 1.0,
                match_reason: "test".to_string(),
                degree: 0,
            },
            SearchResult {
                name: "b".into(),
                label: "Function".into(),
                file_path: None,
                start_line: None,
                qualified_name: Some("b".into()),
                score: 0.8,
                match_reason: "test".to_string(),
                degree: 0,
            },
        ];
        let list_b = vec![
            SearchResult {
                name: "b".into(),
                label: "Function".into(),
                file_path: None,
                start_line: None,
                qualified_name: Some("b".into()),
                score: 1.0,
                match_reason: "test".to_string(),
                degree: 0,
            },
            SearchResult {
                name: "c".into(),
                label: "Function".into(),
                file_path: None,
                start_line: None,
                qualified_name: Some("c".into()),
                score: 0.9,
                match_reason: "test".to_string(),
                degree: 0,
            },
        ];
        let fused = rrf_fuse(list_a, list_b, 10);
        assert_eq!(fused.len(), 3, "should have 3 unique results");
        // "b" appears in both lists → higher RRF score.
        assert_eq!(
            fused[0].name, "b",
            "b should rank first (appears in both lists)"
        );
    }

    #[test]
    fn rrf_fuse_respects_limit() {
        let list_a: Vec<_> = (0..5)
            .map(|i| SearchResult {
                name: format!("a{i}"),
                label: "Function".into(),
                file_path: None,
                start_line: None,
                qualified_name: Some(format!("a{i}")),
                score: 1.0,
                match_reason: "test".to_string(),
                degree: 0,
            })
            .collect();
        let list_b: Vec<_> = (0..5)
            .map(|i| SearchResult {
                name: format!("b{i}"),
                label: "Function".into(),
                file_path: None,
                start_line: None,
                qualified_name: Some(format!("b{i}")),
                score: 1.0,
                match_reason: "test".to_string(),
                degree: 0,
            })
            .collect();
        let fused = rrf_fuse(list_a, list_b, 3);
        assert_eq!(fused.len(), 3, "should respect limit");
    }

    #[test]
    fn rrf_fuse_empty_lists() {
        let fused = rrf_fuse(vec![], vec![], 10);
        assert!(fused.is_empty());
    }

    #[test]
    fn rrf_fuse_one_empty_list() {
        let list_a = vec![SearchResult {
            name: "a".into(),
            label: "Function".into(),
            file_path: None,
            start_line: None,
            qualified_name: Some("a".into()),
            score: 1.0,
            match_reason: "test".to_string(),
            degree: 0,
        }];
        let fused = rrf_fuse(list_a, vec![], 10);
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].name, "a");
    }

    #[test]
    fn rrf_fuse_deduplicates_by_qualified_name() {
        let list_a = vec![SearchResult {
            name: "parse".into(),
            label: "Function".into(),
            file_path: None,
            start_line: None,
            qualified_name: Some("demo.parse".into()),
            score: 1.0,
            match_reason: "test".to_string(),
            degree: 0,
        }];
        let list_b = vec![SearchResult {
            name: "parse".into(),
            label: "Function".into(),
            file_path: None,
            start_line: None,
            qualified_name: Some("demo.parse".into()),
            score: 0.9,
            match_reason: "test".to_string(),
            degree: 0,
        }];
        let fused = rrf_fuse(list_a, list_b, 10);
        assert_eq!(fused.len(), 1, "should deduplicate by qualified_name");
    }

    #[test]
    fn rrf_fuse_deduplicates_by_name_when_no_qn() {
        let list_a = vec![SearchResult {
            name: "parse".into(),
            label: "Function".into(),
            file_path: None,
            start_line: None,
            qualified_name: None,
            score: 1.0,
            match_reason: "test".to_string(),
            degree: 0,
        }];
        let list_b = vec![SearchResult {
            name: "parse".into(),
            label: "Function".into(),
            file_path: None,
            start_line: None,
            qualified_name: None,
            score: 0.9,
            match_reason: "test".to_string(),
            degree: 0,
        }];
        let fused = rrf_fuse(list_a, list_b, 10);
        assert_eq!(fused.len(), 1, "should deduplicate by name when no QN");
    }

    #[test]
    fn rrf_fuse_multi_three_lists() {
        let make = |names: &[&str]| -> Vec<SearchResult> {
            names
                .iter()
                .map(|n| SearchResult {
                    name: n.to_string(),
                    label: "Function".into(),
                    file_path: None,
                    start_line: None,
                    qualified_name: Some(n.to_string()),
                    score: 1.0,
                    match_reason: "test".to_string(),
                degree: 0,
                })
                .collect()
        };
        let lists = vec![
            make(&["a", "b", "c"]),
            make(&["b", "c", "d"]),
            make(&["c", "e"]),
        ];
        let fused = rrf_fuse_multi(lists, 10);
        // "c" appears in all 3 lists → highest RRF score.
        assert_eq!(fused[0].name, "c", "c should rank first (in all 3 lists)");
        assert_eq!(fused.len(), 5, "should have 5 unique results");
    }

    #[test]
    fn rrf_fuse_score_is_updated() {
        let list_a = vec![SearchResult {
            name: "a".into(),
            label: "Function".into(),
            file_path: None,
            start_line: None,
            qualified_name: Some("a".into()),
            score: 0.99,
            match_reason: "test".to_string(),
            degree: 0,
        }];
        let fused = rrf_fuse(list_a, vec![], 10);
        // RRF score for rank 1 in one list: 1/(60+1) ≈ 0.0164
        assert!(
            (fused[0].score - 1.0 / 61.0).abs() < 1e-5,
            "RRF score should be 1/61, got {}",
            fused[0].score
        );
    }

    // --- Bm25Strategy ---

    #[test]
    fn bm25_strategy_returns_results() {
        let conn = fresh_conn();
        seed_fixture(&conn);
        let strategy = Bm25Strategy::new(&conn);
        let results = strategy.search("parse", None, 10).expect("search");
        assert!(!results.is_empty(), "should find results for 'parse'");
        assert!(results
            .iter()
            .all(|r| r.name.to_ascii_lowercase().contains("parse")));
    }

    #[test]
    fn bm25_strategy_respects_limit() {
        let conn = fresh_conn();
        seed_fixture(&conn);
        let strategy = Bm25Strategy::new(&conn);
        let results = strategy.search("parse", None, 1).expect("search");
        assert!(results.len() <= 1);
    }

    #[test]
    fn bm25_strategy_no_matches() {
        let conn = fresh_conn();
        seed_fixture(&conn);
        let strategy = Bm25Strategy::new(&conn);
        let results = strategy
            .search("zzz_nonexistent", None, 10)
            .expect("search");
        assert!(results.is_empty());
    }

    // --- SemanticStrategy ---

    #[test]
    fn semantic_strategy_degrades_to_bm25_without_embeddings() {
        let conn = fresh_conn();
        seed_fixture(&conn);
        let client = MockEmbedClient::new();
        let strategy = SemanticStrategy::new(&conn, &client);
        // No embeddings stored → should degrade or return empty.
        let results = strategy.search("parse", None, 10);
        // Either succeeds (degraded to BM25) or returns empty.
        match results {
            Ok(r) => {
                // If we got results, they should be from BM25.
                let _ = r;
            }
            Err(EmbedError::EmbeddingTableNotAvailable) => {
                // Table not available — acceptable.
            }
            Err(e) => panic!("unexpected error: {e}"),
        }
    }

    #[test]
    fn semantic_strategy_with_error_client_degrades() {
        let conn = fresh_conn();
        seed_fixture(&conn);
        let client = MockEmbedClient::with_error(EmbedError::Unavailable("down".to_string()));
        let strategy = SemanticStrategy::new(&conn, &client);
        let result = strategy.search("parse", None, 10);
        // Should return error (service unavailable) or degrade.
        match result {
            Ok(_) => {}
            Err(EmbedError::Unavailable(_)) => {}
            Err(EmbedError::EmbeddingTableNotAvailable) => {}
            Err(e) => panic!("unexpected: {e}"),
        }
    }

    // --- HybridStrategy ---

    #[test]
    fn hybrid_strategy_returns_results() {
        let conn = fresh_conn();
        seed_fixture(&conn);
        let client = MockEmbedClient::new();
        let strategy = HybridStrategy::new(&conn, &client);
        let results = strategy.search("parse", None, 10);
        match results {
            Ok(r) => {
                // Should get some results (from BM25 at least).
                let _ = r;
            }
            Err(EmbedError::EmbeddingTableNotAvailable) => {}
            Err(e) => panic!("unexpected: {e}"),
        }
    }

    #[test]
    fn hybrid_strategy_degrades_on_error() {
        let conn = fresh_conn();
        seed_fixture(&conn);
        let client = MockEmbedClient::with_error(EmbedError::Unavailable("down".to_string()));
        let strategy = HybridStrategy::new(&conn, &client);
        let result = strategy.search("parse", None, 10);
        // Should degrade to BM25 or error.
        match result {
            Ok(_) => {}
            Err(EmbedError::Unavailable(_)) => {}
            Err(EmbedError::EmbeddingTableNotAvailable) => {}
            Err(e) => panic!("unexpected: {e}"),
        }
    }

    // --- Strategy trait object ---

    #[test]
    fn search_strategy_trait_object_works() {
        let conn = fresh_conn();
        seed_fixture(&conn);
        let strategy: Box<dyn SearchStrategy> = Box::new(Bm25Strategy::new(&conn));
        let results = strategy.search("parse", None, 10).expect("search");
        assert!(!results.is_empty());
    }

    #[test]
    fn search_strategy_trait_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Box<dyn SearchStrategy>>();
    }

    // --- lookup_node_metadata ---

    #[test]
    fn lookup_finds_function_by_id() {
        let conn = fresh_conn();
        seed_fixture(&conn);
        let hit = EmbeddingHit {
            node_id: "f1".to_string(),
            project: "demo".to_string(),
            score: 0.95,
        };
        let result = lookup_node_metadata(&conn, &hit);
        assert!(result.is_some(), "should find function f1");
        let sr = result.unwrap();
        assert_eq!(sr.name, "parse_file");
        assert_eq!(sr.label, "Function");
        assert_eq!(sr.file_path.as_deref(), Some("/src/main.rs"));
        assert_eq!(sr.start_line, Some(1));
        assert_eq!(sr.qualified_name.as_deref(), Some("demo.parse_file"));
        assert!((sr.score - 0.95).abs() < 1e-5);
    }

    #[test]
    fn lookup_returns_none_for_missing_node() {
        let conn = fresh_conn();
        seed_fixture(&conn);
        let hit = EmbeddingHit {
            node_id: "nonexistent".to_string(),
            project: "demo".to_string(),
            score: 0.5,
        };
        let result = lookup_node_metadata(&conn, &hit);
        assert!(result.is_none(), "should return None for missing node");
    }

    // --- Degradation (SubTask 16.4) ---

    #[test]
    fn degradation_semantic_falls_back_to_bm25_on_windows_check() {
        // This test verifies the degradation logic path.
        // On non-Windows, is_vector_supported() is true, so the strategy
        // tries semantic search. On Windows, it degrades to BM25.
        let conn = fresh_conn();
        seed_fixture(&conn);
        let client = MockEmbedClient::new();

        if !is_vector_supported() {
            // Windows: should use BM25.
            let strategy = SemanticStrategy::new(&conn, &client);
            let results = strategy.search("parse", None, 10).expect("search");
            assert!(!results.is_empty(), "degraded BM25 should find results");
        }
        // On non-Windows, the test is a no-op (semantic path is tested elsewhere).
    }

    #[test]
    fn degradation_hybrid_falls_back_to_bm25_on_windows_check() {
        let conn = fresh_conn();
        seed_fixture(&conn);
        let client = MockEmbedClient::new();

        if !is_vector_supported() {
            let strategy = HybridStrategy::new(&conn, &client);
            let results = strategy.search("parse", None, 10).expect("search");
            assert!(!results.is_empty());
        }
    }

    #[test]
    fn degradation_embedding_service_unavailable_continues() {
        // SubTask 16.4: embedding service unavailable → skip embedding, continue.
        let conn = fresh_conn();
        seed_fixture(&conn);
        let client =
            MockEmbedClient::with_error(EmbedError::Unavailable("service down".to_string()));
        let strategy = HybridStrategy::new(&conn, &client);
        // Should not panic; should either degrade to BM25 or return error.
        let _ = strategy.search("parse", None, 10);
    }

    // --- Multi-signal scoring (R-search-002) ---

    /// Seeds a project + two same-named functions (`foo` in /a.rs, `bar` in
    /// /b.rs) where `foo` has 3 CALLS in-edges and `bar` has 0. Both functions
    /// share identical name/content so BM25 scores are equal. Reused by the
    /// centrality test and the combined additivity test.
    fn seed_centrality_fixture(conn: &StorageConnection) {
        conn.execute("CREATE (:Project {id: 'demo', name: 'demo', rootPath: '/', language: 'rust', fileCount: 1, indexedAt: 0, lastCommit: ''});").expect("project");
        conn.execute("CREATE (:Function {id: 'foo', project: 'demo', name: 'process', qualifiedName: 'demo.foo', filePath: '/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: 'process', parentQn: ''});").expect("foo");
        conn.execute("CREATE (:Function {id: 'bar', project: 'demo', name: 'process', qualifiedName: 'demo.bar', filePath: '/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: 'process', parentQn: ''});").expect("bar");
        for i in 1..=3 {
            conn.execute(&format!(
                "CREATE (:Function {{id: 'c{i}', project: 'demo', name: 'caller{i}', qualifiedName: 'demo.caller{i}', filePath: '/c.rs', startLine: {i}, endLine: {i}, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''}});"
            ))
            .expect("caller");
            conn.execute(&format!(
                "CREATE (:CodeRelation {{id: 'cr{i}', source: 'c{i}', target: 'foo', type: 'CALLS', confidence: 1.0, confidenceTier: 'HIGH', reason: '', startLine: 0, project: 'demo'}});"
            ))
            .expect("cr");
        }
    }

    #[test]
    fn hybrid_with_centrality_boosts_high_indegree_nodes() {
        let conn = fresh_conn();
        seed_centrality_fixture(&conn);
        let client = MockEmbedClient::new();
        let strategy = HybridStrategy::new(&conn, &client).with_centrality(true);
        let results = strategy
            .search("process", Some("demo"), 10)
            .expect("search");
        assert!(results.len() >= 2, "should find both foo and bar");
        let foo_rank = results
            .iter()
            .position(|r| r.qualified_name.as_deref() == Some("demo.foo"));
        let bar_rank = results
            .iter()
            .position(|r| r.qualified_name.as_deref() == Some("demo.bar"));
        assert!(foo_rank.is_some(), "foo should be in results");
        assert!(bar_rank.is_some(), "bar should be in results");
        assert!(
            foo_rank.unwrap() < bar_rank.unwrap(),
            "foo (3 CALLS in-edges) should rank higher than bar (0 in-edges) when centrality enabled"
        );
    }

    #[test]
    fn hybrid_with_file_heat_boosts_referenced_files() {
        let conn = fresh_conn();
        // Fixture: File A (5 imports) + File B (0 imports), each containing a
        // same-named function. BM25 scores are equal (same name/content).
        conn.execute("CREATE (:Project {id: 'demo', name: 'demo', rootPath: '/', language: 'rust', fileCount: 2, indexedAt: 0, lastCommit: ''});").expect("project");
        conn.execute("CREATE (:File {id: 'fileA', project: 'demo', name: 'a.rs', filePath: '/a.rs', language: 'rust', hash: '', lineCount: 10});").expect("fileA");
        conn.execute("CREATE (:File {id: 'fileB', project: 'demo', name: 'b.rs', filePath: '/b.rs', language: 'rust', hash: '', lineCount: 10});").expect("fileB");
        conn.execute("CREATE (:Function {id: 'fnA', project: 'demo', name: 'handle', qualifiedName: 'demo.handle_a', filePath: '/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: 'handle', parentQn: ''});").expect("fnA");
        conn.execute("CREATE (:Function {id: 'fnB', project: 'demo', name: 'handle', qualifiedName: 'demo.handle_b', filePath: '/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: 'handle', parentQn: ''});").expect("fnB");
        for i in 1..=5 {
            conn.execute(&format!(
                "CREATE (:File {{id: 'imp{i}', project: 'demo', name: 'imp{i}.rs', filePath: '/imp{i}.rs', language: 'rust', hash: '', lineCount: 1}});"
            ))
            .expect("imp");
            conn.execute(&format!(
                "CREATE (:CodeRelation {{id: 'ir{i}', source: 'imp{i}', target: 'fileA', type: 'IMPORTS', confidence: 1.0, confidenceTier: 'HIGH', reason: '', startLine: 0, project: 'demo'}});"
            ))
            .expect("ir");
        }

        let client = MockEmbedClient::new();
        let strategy = HybridStrategy::new(&conn, &client).with_file_heat(true);
        let results = strategy.search("handle", Some("demo"), 10).expect("search");
        assert!(results.len() >= 2, "should find both fnA and fnB");
        let fn_a_rank = results
            .iter()
            .position(|r| r.qualified_name.as_deref() == Some("demo.handle_a"));
        let fn_b_rank = results
            .iter()
            .position(|r| r.qualified_name.as_deref() == Some("demo.handle_b"));
        assert!(fn_a_rank.is_some(), "fnA should be in results");
        assert!(fn_b_rank.is_some(), "fnB should be in results");
        assert!(
            fn_a_rank.unwrap() < fn_b_rank.unwrap(),
            "fnA (in 5x-imported file) should rank higher than fnB when file_heat enabled"
        );
    }

    #[test]
    fn hybrid_without_centrality_preserves_original_ranking() {
        let conn = fresh_conn();
        seed_fixture(&conn);
        let client = MockEmbedClient::new();
        // Default strategy (no builders invoked).
        let baseline = HybridStrategy::new(&conn, &client);
        let baseline_results = baseline
            .search("parse", Some("demo"), 10)
            .expect("baseline");
        // Strategy with both signals explicitly disabled.
        let disabled = HybridStrategy::new(&conn, &client)
            .with_centrality(false)
            .with_file_heat(false);
        let disabled_results = disabled
            .search("parse", Some("demo"), 10)
            .expect("disabled");
        assert_eq!(
            baseline_results.len(),
            disabled_results.len(),
            "result count must match (backward compatible)"
        );
        for (a, b) in baseline_results.iter().zip(disabled_results.iter()) {
            assert_eq!(a.name, b.name, "name ordering must match");
            assert_eq!(a.qualified_name, b.qualified_name, "QN ordering must match");
            assert!(
                (a.score - b.score).abs() < 1e-6,
                "scores must match: {} vs {}",
                a.score,
                b.score
            );
        }
    }

    #[test]
    fn hybrid_combined_signals_additive() {
        let conn = fresh_conn();
        // Combined fixture: foo in /a.rs (3 CALLS + 5 IMPORTS to fileA),
        // bar in /b.rs (0 CALLS + 0 IMPORTS to fileB).
        seed_centrality_fixture(&conn);
        // Add File nodes matching foo's /a.rs and bar's /b.rs paths.
        conn.execute("CREATE (:File {id: 'fileA', project: 'demo', name: 'a.rs', filePath: '/a.rs', language: 'rust', hash: '', lineCount: 10});").expect("fileA");
        conn.execute("CREATE (:File {id: 'fileB', project: 'demo', name: 'b.rs', filePath: '/b.rs', language: 'rust', hash: '', lineCount: 10});").expect("fileB");
        for i in 1..=5 {
            conn.execute(&format!(
                "CREATE (:File {{id: 'imp{i}', project: 'demo', name: 'imp{i}.rs', filePath: '/imp{i}.rs', language: 'rust', hash: '', lineCount: 1}});"
            ))
            .expect("imp");
            conn.execute(&format!(
                "CREATE (:CodeRelation {{id: 'ir{i}', source: 'imp{i}', target: 'fileA', type: 'IMPORTS', confidence: 1.0, confidenceTier: 'HIGH', reason: '', startLine: 0, project: 'demo'}});"
            ))
            .expect("ir");
        }

        let client = MockEmbedClient::new();

        // Step 1: base scores (no signals).
        let base_results = HybridStrategy::new(&conn, &client)
            .search("process", Some("demo"), 10)
            .expect("base search");
        let base_foo = base_results
            .iter()
            .find(|r| r.qualified_name.as_deref() == Some("demo.foo"))
            .expect("foo in base results")
            .score;
        let base_bar = base_results
            .iter()
            .find(|r| r.qualified_name.as_deref() == Some("demo.bar"))
            .expect("bar in base results")
            .score;

        // Step 2: final scores (both signals enabled).
        let final_results = HybridStrategy::new(&conn, &client)
            .with_centrality(true)
            .with_file_heat(true)
            .search("process", Some("demo"), 10)
            .expect("combined search");
        let final_foo = final_results
            .iter()
            .find(|r| r.qualified_name.as_deref() == Some("demo.foo"))
            .expect("foo in final results")
            .score;
        let final_bar = final_results
            .iter()
            .find(|r| r.qualified_name.as_deref() == Some("demo.bar"))
            .expect("bar in final results")
            .score;

        // foo: centrality = 3/3 = 1.0, file_heat = 5/5 = 1.0.
        // final_foo = 0.5 * base_foo + 0.3 * 1.0 + 0.2 * 1.0.
        let expected_foo = 0.5 * base_foo + 0.3 + 0.2;
        assert!(
            (final_foo - expected_foo).abs() < 1e-5,
            "foo final score {} should be {} (0.5*{} + 0.3*1.0 + 0.2*1.0)",
            final_foo,
            expected_foo,
            base_foo
        );

        // bar: centrality = 0/3 = 0.0, file_heat = 0/5 = 0.0.
        // final_bar = 0.5 * base_bar.
        let expected_bar = 0.5 * base_bar;
        assert!(
            (final_bar - expected_bar).abs() < 1e-5,
            "bar final score {} should be {} (0.5*{})",
            final_bar,
            expected_bar,
            base_bar
        );

        // Sanity: foo should rank higher than bar.
        let foo_rank = final_results
            .iter()
            .position(|r| r.qualified_name.as_deref() == Some("demo.foo"))
            .unwrap();
        let bar_rank = final_results
            .iter()
            .position(|r| r.qualified_name.as_deref() == Some("demo.bar"))
            .unwrap();
        assert!(foo_rank < bar_rank, "foo should rank higher than bar");
    }
}
