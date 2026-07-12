// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Structured search by name / type / file (PRD §4.4.2), plus multi-mode
//! search (exact / regex / fuzzy / graph-enhanced / multi-signal).
//!
//! [`StructuredSearcher`] runs Cypher `MATCH` queries against specific node
//! tables, returning [`SearchResult`] lists. Because LadybugDB stores each
//! [`NodeLabel`] in a distinct table, "search all symbols by name" is
//! implemented as a fan-out across the relevant tables followed by a merge.
//!
//! [`SearchEngine`] provides the multi-mode search dispatcher (T019–T023).

use super::error::{QueryError, Result};
use super::SearchResult;
use crate::model::NodeLabel;
use crate::storage::capability::Storage;
use crate::storage::schema::{escape_cypher_string, escape_identifier};
use crate::storage::StorageConnection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Maximum value accepted for [`SearchParams::limit`].
pub const MAX_LIMIT: usize = 500;
/// Default limit used by [`SearchParams::default`].
pub const DEFAULT_LIMIT: usize = 50;
/// Maximum Levenshtein distance accepted by fuzzy search.
pub const MAX_FUZZY_DISTANCE: usize = 3;

/// Selects which search algorithm [`SearchEngine::search`] dispatches to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchMode {
    /// Exact case-insensitive name match.
    Exact,
    /// Regular-expression match on name / qualifiedName.
    Regex,
    /// Levenshtein-distance fuzzy match.
    Fuzzy,
    /// Name match combined with degree / label graph filters.
    GraphEnhanced,
    /// Multi-signal scored search (name + degree + module + test coverage).
    MultiSignal,
}

/// Parameters for [`SearchEngine::search`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchParams {
    /// Search query (exact text, regex pattern, or fuzzy needle).
    pub query: String,
    /// Match strategy.
    pub mode: SearchMode,
    /// Restrict results to these node labels (e.g. `["Function"]`).
    pub label_filter: Option<Vec<String>>,
    /// `(min, max)` inclusive degree filter for graph-enhanced mode.
    pub degree_filter: Option<(u32, u32)>,
    /// Glob-style file path filter (currently informational).
    pub file_pattern: Option<String>,
    /// Maximum results to return (clamped to [`MAX_LIMIT`]).
    pub limit: usize,
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            query: String::new(),
            mode: SearchMode::Exact,
            label_filter: None,
            degree_filter: None,
            file_pattern: None,
            limit: DEFAULT_LIMIT,
        }
    }
}

impl SearchParams {
    /// Clamps `limit` to `[0, MAX_LIMIT]`.
    pub fn clamped_limit(&self) -> usize {
        self.limit.min(MAX_LIMIT)
    }
}

/// Multi-mode search engine backed by a [`Storage`] capability.
///
/// Dispatches to mode-specific implementations via [`SearchEngine::search`]:
/// - [`SearchMode::Exact`] → name CONTAINS (case-insensitive)
/// - [`SearchMode::Regex`] → [`SearchEngine::search_regex`]
/// - [`SearchMode::Fuzzy`] → [`SearchEngine::search_fuzzy`]
/// - [`SearchMode::GraphEnhanced`] → [`SearchEngine::search_graph_enhanced`]
/// - [`SearchMode::MultiSignal`] → graph-enhanced + [`SearchEngine::score_multi_signal`]
pub struct SearchEngine<'a> {
    storage: &'a dyn Storage,
}

impl<'a> SearchEngine<'a> {
    /// Creates a new engine borrowing the given storage capability.
    #[must_use]
    pub fn new(storage: &'a dyn Storage) -> Self {
        Self { storage }
    }

    /// Dispatches to the appropriate search implementation based on `params.mode`.
    ///
    /// Results are sorted by descending `score`, then ascending `name`.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::InvalidQuery`] for empty queries, invalid regex,
    /// or `max_distance > MAX_FUZZY_DISTANCE`.
    pub fn search(
        &self,
        project: &str,
        params: &SearchParams,
    ) -> Result<Vec<SearchResult>> {
        let limit = params.clamped_limit();
        let mut results = match params.mode {
            SearchMode::Exact => self.search_exact(project, &params.query, limit)?,
            SearchMode::Regex => self.search_regex(project, &params.query)?,
            SearchMode::Fuzzy => self.search_fuzzy(project, &params.query, MAX_FUZZY_DISTANCE)?,
            SearchMode::GraphEnhanced => self.search_graph_enhanced(project, params)?,
            SearchMode::MultiSignal => {
                let mut hits = self.search_graph_enhanced(project, params)?;
                // A-04: batch-load TESTS targets + qn→node_id map once, then
                // score each candidate via in-memory HashSet/HashMap lookups.
                // Old path fired 17 storage queries per candidate (16 label
                // lookups + 1 TESTS edge count); new path fires 17 total.
                let tested_ids = self.load_tested_node_ids(project).unwrap_or_default();
                let qns: Vec<&str> = hits
                    .iter()
                    .filter_map(|h| h.qualified_name.as_deref())
                    .collect();
                let qn_to_id = self.load_qn_to_node_id_map(project, &qns);
                for hit in &mut hits {
                    hit.score = self.score_multi_signal(hit, params, &tested_ids, &qn_to_id);
                    hit.match_reason = "multi-signal".to_string();
                }
                hits
            }
        };
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.name.cmp(&b.name))
        });
        if limit < results.len() {
            results.truncate(limit);
        }
        Ok(results)
    }

    /// Exact case-insensitive substring search (delegates to storage-level CONTAINS).
    fn search_exact(
        &self,
        project: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        if query.trim().is_empty() {
            return Err(QueryError::InvalidQuery("query must not be empty".to_string()));
        }
        let escaped = escape_cypher_string(query);
        let project_esc = escape_cypher_string(project);
        let mut results = Vec::new();
        for &label in SYMBOL_LABELS {
            let table = escape_identifier(label.table_name());
            let cypher = format!(
                "MATCH (n:{table}) WHERE toLower(n.name) CONTAINS toLower('{escaped}') \
                 AND n.project = '{project_esc}' \
                 RETURN n.name AS name, n.qualifiedName AS qn, n.filePath AS filePath, \
                 n.startLine AS line;"
            );
            match self.storage.query(&cypher) {
                Ok(rows) => results.extend(rows_to_search_results(rows, label, query)),
                Err(_) => continue,
            }
        }
        if limit < results.len() {
            results.truncate(limit);
        }
        Ok(results)
    }

    /// Regex search over Function/Method/Class name and qualifiedName.
    ///
    /// Compiles `pattern` as a Rust `regex::Regex` and matches it against
    /// every `Function`, `Method`, and `Class` node in `project`. Both `name`
    /// and `qualifiedName` are tested (case-sensitive, `is_match` semantics).
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::InvalidQuery`] if `pattern` is not a valid regex.
    fn search_regex(&self, project: &str, pattern: &str) -> Result<Vec<SearchResult>> {
        let re = regex::Regex::new(pattern)
            .map_err(|e| QueryError::InvalidQuery(format!("invalid regex: {e}")))?;
        let project_esc = escape_cypher_string(project);
        let labels = [
            NodeLabel::Function,
            NodeLabel::Method,
            NodeLabel::Class,
        ];
        let mut results = Vec::new();
        for &label in &labels {
            let table = escape_identifier(label.table_name());
            let cypher = format!(
                "MATCH (n:{table}) WHERE n.project = '{project_esc}' \
                 RETURN n.name AS name, n.qualifiedName AS qn, n.filePath AS filePath, \
                 n.startLine AS line;"
            );
            let rows = match self.storage.query(&cypher) {
                Ok(rows) => rows,
                Err(_) => continue,
            };
            for row in rows {
                let Some(name) = row.first().and_then(|v| v.as_str()) else {
                    continue;
                };
                let qn = row.get(1).and_then(|v| v.as_str()).map(String::from);
                let file_path = row.get(2).and_then(|v| v.as_str()).map(String::from);
                let start_line = row
                    .get(3)
                    .and_then(|v| v.as_i64())
                    .and_then(|i| u32::try_from(i).ok());
                let matched_on = if re.is_match(name) {
                    "name"
                } else if qn.as_deref().is_some_and(|q| re.is_match(q)) {
                    "qualifiedName"
                } else {
                    continue;
                };
                results.push(SearchResult {
                    name: name.to_string(),
                    label: label.to_string(),
                    file_path,
                    start_line,
                    qualified_name: qn,
                    score: 1.0,
                    match_reason: format!("regex match on {matched_on}"),
                    degree: 0,
                });
            }
        }
        Ok(results)
    }

    /// Fuzzy search using Levenshtein distance.
    ///
    /// Compares `query` (case-insensitive) against every symbol name in
    /// `project`. Results with `distance <= max_distance` are returned.
    /// `max_distance = 0` is equivalent to exact case-insensitive match.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::InvalidQuery`] if `query` is empty or
    /// `max_distance > MAX_FUZZY_DISTANCE`.
    fn search_fuzzy(
        &self,
        project: &str,
        query: &str,
        max_distance: usize,
    ) -> Result<Vec<SearchResult>> {
        if query.trim().is_empty() {
            return Err(QueryError::InvalidQuery("fuzzy query must not be empty".to_string()));
        }
        if max_distance > MAX_FUZZY_DISTANCE {
            return Err(QueryError::InvalidQuery(format!(
                "max_distance {max_distance} exceeds limit {MAX_FUZZY_DISTANCE}"
            )));
        }
        let query_lower = query.to_ascii_lowercase();
        let project_esc = escape_cypher_string(project);
        let mut results = Vec::new();
        for &label in SYMBOL_LABELS {
            let table = escape_identifier(label.table_name());
            let cypher = format!(
                "MATCH (n:{table}) WHERE n.project = '{project_esc}' \
                 RETURN n.name AS name, n.qualifiedName AS qn, n.filePath AS filePath, \
                 n.startLine AS line;"
            );
            let rows = match self.storage.query(&cypher) {
                Ok(rows) => rows,
                Err(_) => continue,
            };
            for row in rows {
                let Some(name) = row.first().and_then(|v| v.as_str()) else {
                    continue;
                };
                let name_lower = name.to_ascii_lowercase();
                let dist = levenshtein(&query_lower, &name_lower);
                if dist > max_distance {
                    continue;
                }
                let qn = row.get(1).and_then(|v| v.as_str()).map(String::from);
                let file_path = row.get(2).and_then(|v| v.as_str()).map(String::from);
                let start_line = row
                    .get(3)
                    .and_then(|v| v.as_i64())
                    .and_then(|i| u32::try_from(i).ok());
                let max_len = query_lower.len().max(name_lower.len()).max(1);
                let score = 1.0 - (dist as f64 / max_len as f64);
                results.push(SearchResult {
                    name: name.to_string(),
                    label: label.to_string(),
                    file_path,
                    start_line,
                    qualified_name: qn,
                    score,
                    match_reason: format!("fuzzy d={dist}"),
                    degree: 0,
                });
            }
        }
        Ok(results)
    }

    /// Graph-enhanced search: name match + degree filter + label filter.
    ///
    /// Combines case-insensitive name CONTAINS with optional:
    /// - `label_filter`: restrict to specific node labels
    /// - `degree_filter`: `(min, max)` inclusive range on incoming CALLS count
    ///
    /// Score is the name relevance (exact=1.0, prefix=0.8, substring=0.5).
    fn search_graph_enhanced(
        &self,
        project: &str,
        params: &SearchParams,
    ) -> Result<Vec<SearchResult>> {
        if params.query.trim().is_empty() {
            return Err(QueryError::InvalidQuery("query must not be empty".to_string()));
        }
        let project_esc = escape_cypher_string(project);
        let query = &params.query;

        // Determine which labels to search.
        let labels: Vec<NodeLabel> = match &params.label_filter {
            Some(names) => names
                .iter()
                .filter_map(|n| parse_node_label(n))
                .collect(),
            None => SYMBOL_LABELS.to_vec(),
        };
        if labels.is_empty() {
            return Ok(Vec::new());
        }

        // Build CALLS indegree map: target_id → count.
        let degree_map = self.load_calls_indegree(project)?;

        let mut results = Vec::new();
        for &label in &labels {
            let table = escape_identifier(label.table_name());
            let cypher = format!(
                "MATCH (n:{table}) WHERE toLower(n.name) CONTAINS toLower('{escaped_q}') \
                 AND n.project = '{project_esc}' \
                 RETURN n.id AS id, n.name AS name, n.qualifiedName AS qn, \
                 n.filePath AS filePath, n.startLine AS line;",
                escaped_q = escape_cypher_string(query),
            );
            let rows = match self.storage.query(&cypher) {
                Ok(rows) => rows,
                Err(_) => continue,
            };
            for row in rows {
                let Some(name) = row.get(1).and_then(|v| v.as_str()) else {
                    continue;
                };
                let node_id = row.first().and_then(|v| v.as_str()).unwrap_or("").to_string();
                let degree = degree_map.get(&node_id).copied().unwrap_or(0);
                // Apply degree filter.
                if let Some((min, max)) = params.degree_filter {
                    if degree < min || degree > max {
                        continue;
                    }
                }
                let qn = row.get(2).and_then(|v| v.as_str()).map(String::from);
                let file_path = row.get(3).and_then(|v| v.as_str()).map(String::from);
                let start_line = row
                    .get(4)
                    .and_then(|v| v.as_i64())
                    .and_then(|i| u32::try_from(i).ok());
                let (score, reason) = relevance_score_with_reason(name, query);
                results.push(SearchResult {
                    name: name.to_string(),
                    label: label.to_string(),
                    file_path,
                    start_line,
                    qualified_name: qn,
                    score,
                    match_reason: format!("{reason} (degree={degree})"),
                    degree,
                });
            }
        }
        Ok(results)
    }

    /// Loads a map of `target_id → incoming CALLS count` for `project`.
    fn load_calls_indegree(
        &self,
        project: &str,
    ) -> Result<std::collections::HashMap<String, u32>> {
        let project_esc = escape_cypher_string(project);
        let cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type = 'CALLS' AND e.project = '{project_esc}' \
             RETURN e.target AS target;"
        );
        let mut map = std::collections::HashMap::new();
        match self.storage.query(&cypher) {
            Ok(rows) => {
                for row in rows {
                    if let Some(target) = row.first().and_then(|v| v.as_str()) {
                        *map.entry(target.to_string()).or_insert(0) += 1;
                    }
                }
            }
            Err(_) => return Ok(map),
        }
        Ok(map)
    }

    /// Multi-signal score in `[0.0, 1.0]`:
    /// - `name_relevance * 0.4` (exact=1.0, prefix/substring=0.8, no=0.0)
    /// - `degree_centrality * 0.3` (min(degree/100, 1.0))
    /// - `module_proximity * 0.2` (file_pattern match=1.0, else=0.5)
    /// - `test_coverage * 0.1` (has incoming TESTS edges=1.0, else=0.0)
    ///
    /// `tested_ids` and `qn_to_id` are batch-loaded once per MultiSignal
    /// search by [`SearchEngine::search`]; this method performs only
    /// in-memory lookups (A-04 batch refactor).
    fn score_multi_signal(
        &self,
        candidate: &SearchResult,
        params: &SearchParams,
        tested_ids: &HashSet<String>,
        qn_to_id: &HashMap<String, String>,
    ) -> f64 {
        let name_relevance = compute_name_relevance(&candidate.name, &params.query);
        let degree = candidate.degree;
        let degree_centrality = (degree as f64 / 100.0).min(1.0);
        let module_proximity = compute_module_proximity(&candidate.file_path, &params.file_pattern);
        let test_coverage = candidate
            .qualified_name
            .as_ref()
            .and_then(|qn| qn_to_id.get(qn))
            .map_or(0.0, |id| {
                if tested_ids.contains(id) { 1.0 } else { 0.0 }
            });

        name_relevance * 0.4
            + degree_centrality * 0.3
            + module_proximity * 0.2
            + test_coverage * 0.1
    }

    /// Batch-loads the set of node ids that are targets of `TESTS` edges in
    /// `project`. Single Cypher query regardless of candidate count (A-04).
    ///
    /// # Errors
    ///
    /// Returns the underlying [`QueryError`] if the storage query fails
    /// catastrophically; transient per-row parse failures are silently
    /// skipped (consistent with the rest of the search pipeline).
    fn load_tested_node_ids(&self, project: &str) -> Result<HashSet<String>> {
        let project_esc = escape_cypher_string(project);
        let cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type = 'TESTS' AND e.project = '{project_esc}' \
             RETURN e.target AS target;"
        );
        let mut set = HashSet::new();
        match self.storage.query(&cypher) {
            Ok(rows) => {
                for row in rows {
                    if let Some(target) = row.first().and_then(|v| v.as_str()) {
                        set.insert(target.to_string());
                    }
                }
            }
            Err(e) => return Err(e.into()),
        }
        Ok(set)
    }

    /// Batch-resolves `qualifiedName → node_id` for the supplied `qns` in
    /// `project`. Fans out across [`SYMBOL_LABELS`] once per label (16
    /// queries) regardless of candidate count, replacing the old per-
    /// candidate lookup (A-04).
    fn load_qn_to_node_id_map(&self, project: &str, qns: &[&str]) -> HashMap<String, String> {
        if qns.is_empty() {
            return HashMap::new();
        }
        let project_esc = escape_cypher_string(project);
        let qn_list = qns
            .iter()
            .map(|q| format!("'{}'", escape_cypher_string(q)))
            .collect::<Vec<_>>()
            .join(", ");
        let mut map = HashMap::new();
        for &label in SYMBOL_LABELS {
            let table = escape_identifier(label.table_name());
            let cypher = format!(
                "MATCH (n:{table}) WHERE n.qualifiedName IN [{qn_list}] \
                 AND n.project = '{project_esc}' \
                 RETURN n.qualifiedName AS qn, n.id AS id;"
            );
            if let Ok(rows) = self.storage.query(&cypher) {
                for row in rows {
                    let qn = row.first().and_then(|v| v.as_str());
                    let id = row.get(1).and_then(|v| v.as_str());
                    if let (Some(qn), Some(id)) = (qn, id) {
                        map.insert(qn.to_string(), id.to_string());
                    }
                }
            }
        }
        map
    }
}

/// Symbol-bearing node labels searched by [`StructuredSearcher::search`] and
/// [`StructuredSearcher::search_by_name`]. Project/Folder/File/Parameter are
/// excluded: Project has no `project` column, Folder/File are structural, and
/// Parameter is too granular for general symbol search.
const SYMBOL_LABELS: &[NodeLabel] = &[
    NodeLabel::Module,
    NodeLabel::Class,
    NodeLabel::Struct,
    NodeLabel::Enum,
    NodeLabel::Trait,
    NodeLabel::Impl,
    NodeLabel::Function,
    NodeLabel::Method,
    NodeLabel::Variable,
    NodeLabel::GlobalVar,
    NodeLabel::Const,
    NodeLabel::Static,
    NodeLabel::Macro,
    NodeLabel::TypeAlias,
    NodeLabel::Typedef,
    NodeLabel::Namespace,
];

/// Executes structured (name/type/file) searches against a [`StorageConnection`].
pub struct StructuredSearcher<'a> {
    conn: &'a StorageConnection,
}

impl<'a> StructuredSearcher<'a> {
    /// Creates a new [`StructuredSearcher`] borrowing `conn`.
    #[must_use]
    pub fn new(conn: &'a StorageConnection) -> Self {
        Self { conn }
    }

    /// Searches all symbol tables for nodes whose `name` contains `name`.
    ///
    /// Results are merged across tables and sorted by descending relevance
    /// score (exact > prefix > substring), then by name. The `project` filter
    /// is applied when supplied. Matching is case-insensitive.
    pub fn search_by_name(
        &self,
        name: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        if name.trim().is_empty() {
            return Err(QueryError::InvalidQuery(
                "search name must not be empty".to_string(),
            ));
        }
        let escaped = escape_cypher_string(name);
        let mut results = Vec::new();
        for &label in SYMBOL_LABELS {
            let table = escape_identifier(label.table_name());
            // Use toLower() for case-insensitive CONTAINS matching.
            let cypher = match project {
                Some(p) => format!(
                    "MATCH (n:{table}) WHERE toLower(n.name) CONTAINS toLower('{escaped}') AND n.project = '{}' RETURN n.name AS name, n.qualifiedName AS qn, n.filePath AS filePath, n.startLine AS line;",
                    escape_cypher_string(p),
                ),
                None => format!(
                    "MATCH (n:{table}) WHERE toLower(n.name) CONTAINS toLower('{escaped}') RETURN n.name AS name, n.qualifiedName AS qn, n.filePath AS filePath, n.startLine AS line;",
                ),
            };
            // Some tables may lack a `qualifiedName` or `filePath` column, or
            // toLower() may be unsupported; skip those gracefully.
            match self.conn.query(&cypher) {
                Ok(rows) => results.extend(rows_to_search_results(rows, label, name)),
                Err(_) => continue,
            }
        }
        sort_and_truncate(&mut results, limit);
        Ok(results)
    }

    /// Returns all nodes of the given `label`, optionally filtered by project.
    pub fn search_by_type(
        &self,
        label: NodeLabel,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let table = escape_identifier(label.table_name());
        let cypher = match project {
            Some(p) => format!(
                "MATCH (n:{table}) WHERE n.project = '{}' RETURN n.name AS name, n.qualifiedName AS qn, n.filePath AS filePath, n.startLine AS line;",
                escape_cypher_string(p),
            ),
            None => format!(
                "MATCH (n:{table}) RETURN n.name AS name, n.qualifiedName AS qn, n.filePath AS filePath, n.startLine AS line;",
            ),
        };
        let rows = self.conn.query(&cypher)?;
        let mut results = rows_to_search_results(rows, label, "");
        sort_and_truncate(&mut results, limit);
        Ok(results)
    }

    /// Returns all symbols located in the given `file_path`, optionally
    /// filtered by project.
    pub fn search_by_file(
        &self,
        file_path: &str,
        project: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        if file_path.trim().is_empty() {
            return Err(QueryError::InvalidQuery(
                "file path must not be empty".to_string(),
            ));
        }
        let escaped = escape_cypher_string(file_path);
        let mut results = Vec::new();
        for &label in SYMBOL_LABELS {
            let table = escape_identifier(label.table_name());
            let cypher = match project {
                Some(p) => format!(
                    "MATCH (n:{table}) WHERE n.filePath = '{escaped}' AND n.project = '{}' RETURN n.name AS name, n.qualifiedName AS qn, n.filePath AS filePath, n.startLine AS line;",
                    escape_cypher_string(p),
                ),
                None => format!(
                    "MATCH (n:{table}) WHERE n.filePath = '{escaped}' RETURN n.name AS name, n.qualifiedName AS qn, n.filePath AS filePath, n.startLine AS line;",
                ),
            };
            match self.conn.query(&cypher) {
                Ok(rows) => results.extend(rows_to_search_results(rows, label, "")),
                Err(_) => continue,
            }
        }
        // Sort by start line for deterministic file-order output.
        results.sort_by(|a, b| {
            a.start_line
                .unwrap_or(0)
                .cmp(&b.start_line.unwrap_or(0))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(results)
    }

    /// General search: searches by name (CONTAINS) and returns results sorted
    /// by relevance. Equivalent to [`StructuredSearcher::search_by_name`] but
    /// provided as the default `search` entry point for the facade.
    pub fn search(
        &self,
        text: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        self.search_by_name(text, project, limit)
    }
}

/// Converts query rows into [`SearchResult`]s, computing a relevance score
/// based on how closely `name` matches each result's name.
fn rows_to_search_results(
    rows: Vec<Vec<serde_json::Value>>,
    label: NodeLabel,
    query: &str,
) -> Vec<SearchResult> {
    let label_str = label.to_string();
    rows.into_iter()
        .filter_map(|row| {
            let name = row.first().and_then(|v| v.as_str())?.to_string();
            let qualified_name = row.get(1).and_then(|v| v.as_str()).map(String::from);
            let file_path = row.get(2).and_then(|v| v.as_str()).map(String::from);
            let start_line = row
                .get(3)
                .and_then(|v| v.as_i64())
                .and_then(|i| u32::try_from(i).ok());
            let (score, reason) = relevance_score_with_reason(&name, query);
            Some(SearchResult {
                name,
                label: label_str.clone(),
                file_path,
                start_line,
                qualified_name,
                score,
                match_reason: reason.to_string(),
                degree: 0,
            })
        })
        .collect()
}

/// Computes a relevance score in `[0.0, 1.0]` for `name` against `query`.
///
/// - Exact match → 1.0
/// - Prefix match → 0.8
/// - Substring match → 0.5
/// - No query (e.g. `search_by_type`) → 1.0 (neutral)
/// Computes both the score and a human-readable match reason.
fn relevance_score_with_reason(name: &str, query: &str) -> (f64, &'static str) {
    if query.is_empty() {
        return (1.0, "neutral");
    }
    let name_lower = name.to_ascii_lowercase();
    let query_lower = query.to_ascii_lowercase();
    if name_lower == query_lower {
        (1.0, "exact name match")
    } else if name_lower.starts_with(&query_lower) {
        (0.8, "prefix match")
    } else {
        (0.5, "substring match")
    }
}

/// Computes the standard Levenshtein edit distance between `a` and `b`.
///
/// Uses O(`min(a.len, b.len)`) space (single-row DP with the previous
/// diagonal value tracked separately).
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    if a_bytes.is_empty() {
        return b_bytes.len();
    }
    if b_bytes.is_empty() {
        return a_bytes.len();
    }
    // Ensure `b` is the shorter string to minimise row width.
    let (a_bytes, b_bytes) = if a_bytes.len() < b_bytes.len() {
        (b_bytes, a_bytes)
    } else {
        (a_bytes, b_bytes)
    };
    let b_len = b_bytes.len();
    let mut prev_row: Vec<usize> = (0..=b_len).collect();
    let mut curr_row = vec![0usize; b_len + 1];
    for (i, &a_ch) in a_bytes.iter().enumerate() {
        curr_row[0] = i + 1;
        for (j, &b_ch) in b_bytes.iter().enumerate() {
            let cost = usize::from(a_ch != b_ch);
            curr_row[j + 1] = (prev_row[j + 1] + 1)
                .min(curr_row[j] + 1)
                .min(prev_row[j] + cost);
        }
        std::mem::swap(&mut prev_row, &mut curr_row);
    }
    prev_row[b_len]
}

/// Parses a node label name (e.g. `"Function"`, `"function"`) into a
/// [`NodeLabel`]. Returns `None` for unknown labels.
fn parse_node_label(name: &str) -> Option<NodeLabel> {
    name.parse::<NodeLabel>().ok()
}

/// Computes name relevance for multi-signal scoring (R-search-004).
///
/// - Exact case-insensitive match → 1.0
/// - Prefix or substring match → 0.8
/// - No match → 0.0
fn compute_name_relevance(name: &str, query: &str) -> f64 {
    if query.is_empty() {
        return 1.0;
    }
    let name_lower = name.to_ascii_lowercase();
    let query_lower = query.to_ascii_lowercase();
    if name_lower == query_lower {
        1.0
    } else if name_lower.contains(&query_lower) {
        0.8
    } else {
        0.0
    }
}

/// Computes module proximity for multi-signal scoring (R-search-004).
///
/// - `file_pattern` is provided and matches the candidate's `file_path` → 1.0
/// - Otherwise (no pattern or no match) → 0.5
fn compute_module_proximity(
    file_path: &Option<String>,
    file_pattern: &Option<String>,
) -> f64 {
    match (file_path, file_pattern) {
        (Some(path), Some(pattern)) if path.contains(pattern) => 1.0,
        _ => 0.5,
    }
}

/// Sorts results by descending score then ascending name, and truncates to `limit`.
fn sort_and_truncate(results: &mut Vec<SearchResult>, limit: usize) {
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });
    if limit < results.len() {
        results.truncate(limit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Edge, EdgeType, Language, Node, NodeLabel};
    use crate::storage::Repository;

    fn fresh_repo() -> Repository {
        Repository::in_memory().expect("in_memory repository")
    }

    fn sample_function(
        id: &str,
        project: &str,
        name: &str,
        qn: &str,
        file: &str,
        line: u32,
    ) -> Node {
        Node::builder(NodeLabel::Function, name, qn)
            .id(id)
            .project(project)
            .file_path(file)
            .start_line(line)
            .end_line(line + 10)
            .language(Language::Rust)
            .signature("fn x()")
            .build()
    }

    fn sample_class(id: &str, project: &str, name: &str, qn: &str, file: &str, line: u32) -> Node {
        Node::builder(NodeLabel::Class, name, qn)
            .id(id)
            .project(project)
            .file_path(file)
            .start_line(line)
            .end_line(line + 20)
            .language(Language::Rust)
            .build()
    }

    // --- search_by_name ---

    #[test]
    fn search_by_name_finds_substring_matches() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_function("f1", "demo", "parse_file", "demo.parse_file", "/a.rs", 1),
                sample_function("f2", "demo", "read_input", "demo.read_input", "/a.rs", 10),
                sample_function("f3", "demo", "parse_line", "demo.parse_line", "/b.rs", 1),
            ],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_name("parse", None, 100)
            .expect("search_by_name");
        let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"parse_file"));
        assert!(names.contains(&"parse_line"));
        assert!(!names.contains(&"read_input"));
    }

    #[test]
    fn search_by_name_ac_search_001_returns_parse_symbols() {
        // AC-SEARCH-001: search "parse" returns symbols containing "parse".
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_function("f1", "demo", "parse", "demo.parse", "/a.rs", 1),
                sample_function("f2", "demo", "parse_token", "demo.parse_token", "/a.rs", 5),
                sample_function("f3", "demo", "tokenize", "demo.tokenize", "/b.rs", 1),
            ],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher.search_by_name("parse", None, 100).expect("search");
        assert!(results.iter().all(|r| r.name.contains("parse")));
        assert!(results.iter().any(|r| r.name == "parse"));
        assert!(results.iter().any(|r| r.name == "parse_token"));
    }

    #[test]
    fn search_by_name_filters_by_project() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function(
                "f1",
                "alpha",
                "parse",
                "alpha.parse",
                "/a.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes alpha");
        repo.save_nodes(
            &[sample_function(
                "f2",
                "beta",
                "parse",
                "beta.parse",
                "/b.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes beta");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_name("parse", Some("alpha"), 100)
            .expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].qualified_name.as_deref(), Some("alpha.parse"));
    }

    #[test]
    fn search_by_name_respects_limit() {
        let repo = fresh_repo();
        let mut nodes = Vec::new();
        for i in 0..10 {
            nodes.push(sample_function(
                &format!("f{i}"),
                "demo",
                &format!("parse_{i}"),
                &format!("demo.parse_{i}"),
                "/a.rs",
                i + 1,
            ));
        }
        repo.save_nodes(&nodes, NodeLabel::Function)
            .expect("save_nodes");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher.search_by_name("parse", None, 3).expect("search");
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn search_by_name_rejects_empty_query() {
        let repo = fresh_repo();
        let searcher = StructuredSearcher::new(repo.connection());
        let err = searcher
            .search_by_name("", None, 10)
            .expect_err("empty name should error");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn search_by_name_returns_empty_when_no_match() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function(
                "f1",
                "demo",
                "main",
                "demo.main",
                "/a.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_name("nonexistent", None, 10)
            .expect("search");
        assert!(results.is_empty());
    }

    #[test]
    fn search_by_name_searches_across_multiple_labels() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function(
                "f1",
                "demo",
                "parse",
                "demo.parse",
                "/a.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes function");
        repo.save_nodes(
            &[sample_class(
                "c1",
                "demo",
                "Parser",
                "demo.Parser",
                "/a.rs",
                20,
            )],
            NodeLabel::Class,
        )
        .expect("save_nodes class");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher.search_by_name("parse", None, 100).expect("search");
        // Case-insensitive substring: "Parser" contains "parse" lowercased.
        let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"parse"));
        assert!(names.contains(&"Parser"));
    }

    #[test]
    fn search_by_name_assigns_higher_score_to_exact_match() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_function("f1", "demo", "parse", "demo.parse", "/a.rs", 1),
                sample_function("f2", "demo", "parse_file", "demo.parse_file", "/a.rs", 5),
            ],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher.search_by_name("parse", None, 100).expect("search");
        // Exact match should be first (highest score).
        assert_eq!(results[0].name, "parse");
        assert!(results[0].score > results[1].score);
    }

    // --- search_by_type ---

    #[test]
    fn search_by_type_returns_all_functions() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_function("f1", "demo", "alpha", "demo.alpha", "/a.rs", 1),
                sample_function("f2", "demo", "beta", "demo.beta", "/a.rs", 5),
            ],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        repo.save_nodes(
            &[sample_class(
                "c1",
                "demo",
                "Gamma",
                "demo.Gamma",
                "/a.rs",
                10,
            )],
            NodeLabel::Class,
        )
        .expect("save_nodes class");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_type(NodeLabel::Function, None, 100)
            .expect("search_by_type");
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.label == "Function"));
    }

    #[test]
    fn search_by_type_returns_all_classes() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_class("c1", "demo", "Alpha", "demo.Alpha", "/a.rs", 1),
                sample_class("c2", "demo", "Beta", "demo.Beta", "/a.rs", 10),
            ],
            NodeLabel::Class,
        )
        .expect("save_nodes");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_type(NodeLabel::Class, None, 100)
            .expect("search_by_type");
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.label == "Class"));
    }

    #[test]
    fn search_by_type_filters_by_project() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function(
                "f1",
                "alpha",
                "main",
                "alpha.main",
                "/a.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes alpha");
        repo.save_nodes(
            &[
                sample_function("f2", "beta", "main", "beta.main", "/a.rs", 1),
                sample_function("f3", "beta", "util", "beta.util", "/a.rs", 5),
            ],
            NodeLabel::Function,
        )
        .expect("save_nodes beta");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_type(NodeLabel::Function, Some("beta"), 100)
            .expect("search_by_type");
        assert_eq!(results.len(), 2);
        assert!(results
            .iter()
            .all(|r| r.qualified_name.as_ref().unwrap().starts_with("beta.")));
    }

    #[test]
    fn search_by_type_respects_limit() {
        let repo = fresh_repo();
        let mut nodes = Vec::new();
        for i in 0..10 {
            nodes.push(sample_function(
                &format!("f{i}"),
                "demo",
                &format!("func_{i}"),
                &format!("demo.func_{i}"),
                "/a.rs",
                i + 1,
            ));
        }
        repo.save_nodes(&nodes, NodeLabel::Function)
            .expect("save_nodes");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_type(NodeLabel::Function, None, 3)
            .expect("search_by_type");
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn search_by_type_returns_empty_when_none() {
        let repo = fresh_repo();
        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_type(NodeLabel::Function, None, 100)
            .expect("search_by_type");
        assert!(results.is_empty());
    }

    // --- search_by_file ---

    #[test]
    fn search_by_file_returns_all_symbols_in_file() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[
                sample_function("f1", "demo", "alpha", "demo.alpha", "/src/main.rs", 1),
                sample_function("f2", "demo", "beta", "demo.beta", "/src/main.rs", 10),
            ],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        repo.save_nodes(
            &[sample_class(
                "c1",
                "demo",
                "Gamma",
                "demo.Gamma",
                "/src/main.rs",
                20,
            )],
            NodeLabel::Class,
        )
        .expect("save_nodes class");
        // A symbol in a different file should not appear.
        repo.save_nodes(
            &[sample_function(
                "f3",
                "demo",
                "delta",
                "demo.delta",
                "/src/lib.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes other file");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_file("/src/main.rs", None)
            .expect("search_by_file");
        assert_eq!(results.len(), 3);
        assert!(results
            .iter()
            .all(|r| r.file_path.as_deref() == Some("/src/main.rs")));
    }

    #[test]
    fn search_by_file_filters_by_project() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function(
                "f1",
                "alpha",
                "main",
                "alpha.main",
                "/src/main.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes alpha");
        repo.save_nodes(
            &[sample_function(
                "f2",
                "beta",
                "main",
                "beta.main",
                "/src/main.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes beta");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_file("/src/main.rs", Some("alpha"))
            .expect("search_by_file");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].qualified_name.as_deref(), Some("alpha.main"));
    }

    #[test]
    fn search_by_file_rejects_empty_path() {
        let repo = fresh_repo();
        let searcher = StructuredSearcher::new(repo.connection());
        let err = searcher
            .search_by_file("", None)
            .expect_err("empty path should error");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn search_by_file_returns_empty_when_no_match() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function(
                "f1",
                "demo",
                "main",
                "demo.main",
                "/a.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_file("/nonexistent.rs", None)
            .expect("search_by_file");
        assert!(results.is_empty());
    }

    // --- search (general) ---

    #[test]
    fn search_delegates_to_search_by_name() {
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function(
                "f1",
                "demo",
                "parse_file",
                "demo.parse_file",
                "/a.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes");

        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher.search("parse", None, 100).expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "parse_file");
    }

    // --- helpers ---

    #[test]
    fn relevance_score_exact_match_is_highest() {
        assert_eq!(relevance_score_with_reason("parse", "parse").0, 1.0);
        assert_eq!(relevance_score_with_reason("PARSE", "parse").0, 1.0);
    }

    #[test]
    fn relevance_score_prefix_match_is_high() {
        assert_eq!(relevance_score_with_reason("parse_file", "parse").0, 0.8);
    }

    #[test]
    fn relevance_score_substring_match_is_low() {
        assert_eq!(relevance_score_with_reason("my_parse_func", "parse").0, 0.5);
    }

    #[test]
    fn relevance_score_empty_query_is_neutral() {
        assert_eq!(relevance_score_with_reason("anything", "").0, 1.0);
    }

    // --- error continuation coverage ---

    #[test]
    fn search_by_name_continues_on_query_error() {
        // Cover the `Err(_) => continue` arm (line 86) of search_by_name:
        // when a per-table MATCH query fails (table dropped), the loop
        // skips it and continues to the next table.
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function(
                "f1",
                "demo",
                "parse",
                "demo.parse",
                "/a.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        // Drop the Class table so its MATCH query errors.
        repo.connection()
            .execute("DROP TABLE Class;")
            .expect("drop table");
        // Verify the table is actually gone.
        let check = repo
            .connection()
            .query("MATCH (n:Class) RETURN n.name AS name;");
        assert!(check.is_err(), "Class table should be gone after DROP");
        let searcher = StructuredSearcher::new(repo.connection());
        // Should still return results from the Function table.
        let results = searcher.search_by_name("parse", None, 100).expect("search");
        assert!(results.iter().any(|r| r.name == "parse"));
    }

    #[test]
    fn search_by_file_continues_on_query_error() {
        // Cover the `Err(_) => continue` arm (line 143) of search_by_file.
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function(
                "f1",
                "demo",
                "parse",
                "demo.parse",
                "/src/main.rs",
                1,
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        repo.connection()
            .execute("DROP TABLE Class;")
            .expect("drop table");
        let searcher = StructuredSearcher::new(repo.connection());
        let results = searcher
            .search_by_file("/src/main.rs", None)
            .expect("search_by_file");
        assert!(results.iter().any(|r| r.name == "parse"));
    }

    // --- T019: multi-mode search types ---

    use crate::kit::StorageModule;
    use crate::storage::StorageConfig;

    fn build_storage() -> std::sync::Arc<dyn Storage> {
        StorageModule::build_cap(&StorageConfig::in_memory()).expect("StorageModule::build_cap")
    }

    #[test]
    fn search_mode_serializes_to_descriptive_strings() {
        let json = serde_json::to_string(&SearchMode::Exact).expect("serialize Exact");
        assert_eq!(json, "\"Exact\"");
        let json = serde_json::to_string(&SearchMode::Regex).expect("serialize Regex");
        assert_eq!(json, "\"Regex\"");
        let json = serde_json::to_string(&SearchMode::Fuzzy).expect("serialize Fuzzy");
        assert_eq!(json, "\"Fuzzy\"");
        let json =
            serde_json::to_string(&SearchMode::GraphEnhanced).expect("serialize GraphEnhanced");
        assert_eq!(json, "\"GraphEnhanced\"");
        let json =
            serde_json::to_string(&SearchMode::MultiSignal).expect("serialize MultiSignal");
        assert_eq!(json, "\"MultiSignal\"");
    }

    #[test]
    fn search_mode_round_trips_through_json() {
        for mode in [
            SearchMode::Exact,
            SearchMode::Regex,
            SearchMode::Fuzzy,
            SearchMode::GraphEnhanced,
            SearchMode::MultiSignal,
        ] {
            let json = serde_json::to_string(&mode).expect("serialize");
            let back: SearchMode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(mode, back);
        }
    }

    #[test]
    fn search_params_default_has_limit_50_and_exact_mode() {
        let p = SearchParams::default();
        assert_eq!(p.limit, 50);
        assert_eq!(p.mode, SearchMode::Exact);
        assert!(p.label_filter.is_none());
        assert!(p.degree_filter.is_none());
        assert!(p.file_pattern.is_none());
    }

    #[test]
    fn search_params_clamps_limit_to_max_500() {
        let mut p = SearchParams::default();
        p.limit = 1000;
        assert_eq!(p.clamped_limit(), MAX_LIMIT);
        p.limit = 10;
        assert_eq!(p.clamped_limit(), 10);
    }

    #[test]
    fn search_params_round_trips_through_json() {
        let p = SearchParams {
            query: "handler".to_string(),
            mode: SearchMode::GraphEnhanced,
            label_filter: Some(vec!["Function".to_string()]),
            degree_filter: Some((5, 100)),
            file_pattern: Some("/src/**/*.rs".to_string()),
            limit: 25,
        };
        let json = serde_json::to_string(&p).expect("serialize");
        let back: SearchParams = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(p, back);
    }

    #[test]
    fn search_engine_dispatches_to_exact_mode() {
        let storage = build_storage();
        let func = Node::builder(NodeLabel::Function, "parse_file", "demo.parse_file")
            .id("f1")
            .project("demo")
            .file_path("/a.rs")
            .start_line(1)
            .end_line(10)
            .language(Language::Rust)
            .build();
        storage.save_nodes(&[func], NodeLabel::Function).expect("save_nodes");

        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: "parse".to_string(),
            mode: SearchMode::Exact,
            limit: 50,
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("exact search");
        assert!(results.iter().any(|r| r.name == "parse_file"));
        assert!(results.iter().all(|r| r.match_reason.contains("match")));
    }

    #[test]
    fn search_engine_exact_rejects_empty_query() {
        let storage = build_storage();
        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: String::new(),
            mode: SearchMode::Exact,
            ..SearchParams::default()
        };
        let err = engine.search("demo", &params).expect_err("empty query should error");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn search_engine_regex_matches_pattern_on_name() {
        let storage = build_storage();
        let funcs = [
            Node::builder(NodeLabel::Function, "get_user_by_id", "demo.get_user_by_id")
                .id("f1")
                .project("demo")
                .file_path("/a.rs")
                .start_line(1)
                .end_line(10)
                .language(Language::Rust)
                .build(),
            Node::builder(NodeLabel::Function, "get_first_user", "demo.get_first_user")
                .id("f2")
                .project("demo")
                .file_path("/a.rs")
                .start_line(20)
                .end_line(30)
                .language(Language::Rust)
                .build(),
            Node::builder(NodeLabel::Function, "delete_user", "demo.delete_user")
                .id("f3")
                .project("demo")
                .file_path("/a.rs")
                .start_line(40)
                .end_line(50)
                .language(Language::Rust)
                .build(),
        ];
        storage.save_nodes(&funcs, NodeLabel::Function).expect("save_nodes");

        let engine = SearchEngine::new(storage.as_ref());
        // Pattern `get_.*_user` matches "get_first_user" but not "get_user_by_id"
        // (no "_user" substring after "get_"). Use `get_.*` to match both.
        let params = SearchParams {
            query: r"get_.*_user".to_string(),
            mode: SearchMode::Regex,
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("regex search");
        let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"get_first_user"), "should match get_first_user, got {names:?}");
        assert!(!names.contains(&"delete_user"));
        assert!(results.iter().all(|r| r.match_reason.starts_with("regex match")));

        // Broader pattern matches both get_* functions.
        let params2 = SearchParams {
            query: r"get_.*".to_string(),
            mode: SearchMode::Regex,
            ..SearchParams::default()
        };
        let results2 = engine.search("demo", &params2).expect("regex search");
        let names2: Vec<&str> = results2.iter().map(|r| r.name.as_str()).collect();
        assert!(names2.contains(&"get_user_by_id"));
        assert!(names2.contains(&"get_first_user"));
        assert!(!names2.contains(&"delete_user"));
    }

    #[test]
    fn search_engine_regex_matches_suffix_pattern() {
        let storage = build_storage();
        let classes = [
            Node::builder(NodeLabel::Class, "RequestHandler", "demo.RequestHandler")
                .id("c1")
                .project("demo")
                .file_path("/a.rs")
                .start_line(1)
                .end_line(50)
                .language(Language::Rust)
                .build(),
            Node::builder(NodeLabel::Class, "BaseController", "demo.BaseController")
                .id("c2")
                .project("demo")
                .file_path("/a.rs")
                .start_line(60)
                .end_line(100)
                .language(Language::Rust)
                .build(),
        ];
        storage.save_nodes(&classes, NodeLabel::Class).expect("save_nodes");

        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: r"Handler$".to_string(),
            mode: SearchMode::Regex,
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("regex search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "RequestHandler");
    }

    #[test]
    fn search_engine_regex_matches_on_qualified_name() {
        let storage = build_storage();
        let func = Node::builder(NodeLabel::Function, "do_work", "demo.handler.process")
                .id("f1")
                .project("demo")
                .file_path("/a.rs")
                .start_line(1)
                .end_line(10)
                .language(Language::Rust)
                .build();
        storage.save_nodes(&[func], NodeLabel::Function).expect("save_nodes");

        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: r"^demo\.handler".to_string(),
            mode: SearchMode::Regex,
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("regex search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "do_work");
        assert!(results[0].match_reason.contains("qualifiedName"));
    }

    #[test]
    fn search_engine_regex_rejects_invalid_pattern() {
        let storage = build_storage();
        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: r"[invalid".to_string(),
            mode: SearchMode::Regex,
            ..SearchParams::default()
        };
        let err = engine.search("demo", &params).expect_err("invalid regex should error");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn search_engine_regex_returns_empty_when_no_match() {
        let storage = build_storage();
        let func = Node::builder(NodeLabel::Function, "main", "demo.main")
                .id("f1")
                .project("demo")
                .file_path("/a.rs")
                .start_line(1)
                .end_line(10)
                .language(Language::Rust)
                .build();
        storage.save_nodes(&[func], NodeLabel::Function).expect("save_nodes");

        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: r"nonexistent_\d+".to_string(),
            mode: SearchMode::Regex,
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("regex search");
        assert!(results.is_empty());
    }

    #[test]
    fn search_engine_fuzzy_matches_within_distance() {
        let storage = build_storage();
        let funcs = [
            Node::builder(NodeLabel::Function, "get_user", "demo.get_user")
                .id("f1")
                .project("demo")
                .file_path("/a.rs")
                .start_line(1)
                .end_line(10)
                .language(Language::Rust)
                .build(),
            Node::builder(NodeLabel::Function, "getUser", "demo.getUser")
                .id("f2")
                .project("demo")
                .file_path("/a.rs")
                .start_line(20)
                .end_line(30)
                .language(Language::Rust)
                .build(),
            Node::builder(NodeLabel::Function, "completely_unrelated", "demo.completely_unrelated")
                .id("f3")
                .project("demo")
                .file_path("/a.rs")
                .start_line(40)
                .end_line(50)
                .language(Language::Rust)
                .build(),
        ];
        storage.save_nodes(&funcs, NodeLabel::Function).expect("save_nodes");

        let engine = SearchEngine::new(storage.as_ref());
        // "getuser" vs "get_user" → distance 1 (insert "_")
        // "getuser" vs "getUser"  → distance 0 (case-insensitive equal)
        let results = engine
            .search_fuzzy("demo", "getuser", 2)
            .expect("fuzzy search");
        let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"get_user"), "should match get_user, got {names:?}");
        assert!(names.contains(&"getUser"), "should match getUser, got {names:?}");
        assert!(!names.contains(&"completely_unrelated"));
        assert!(results.iter().all(|r| r.match_reason.starts_with("fuzzy")));
    }

    #[test]
    fn search_engine_fuzzy_max_distance_zero_is_exact() {
        let storage = build_storage();
        let funcs = [
            Node::builder(NodeLabel::Function, "fetch", "demo.fetch")
                .id("f1")
                .project("demo")
                .file_path("/a.rs")
                .start_line(1)
                .end_line(10)
                .language(Language::Rust)
                .build(),
            Node::builder(NodeLabel::Function, "fetcher", "demo.fetcher")
                .id("f2")
                .project("demo")
                .file_path("/a.rs")
                .start_line(20)
                .end_line(30)
                .language(Language::Rust)
                .build(),
        ];
        storage.save_nodes(&funcs, NodeLabel::Function).expect("save_nodes");

        let engine = SearchEngine::new(storage.as_ref());
        // max_distance=0 → exact case-insensitive match only
        let results = engine
            .search_fuzzy("demo", "FETCH", 0)
            .expect("fuzzy search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "fetch");
        assert_eq!(results[0].match_reason, "fuzzy d=0");
    }

    #[test]
    fn search_engine_fuzzy_excludes_beyond_max_distance() {
        let storage = build_storage();
        // "fethc" vs "fetch" → standard Levenshtein distance = 2
        let func = Node::builder(NodeLabel::Function, "fetch", "demo.fetch")
                .id("f1")
                .project("demo")
                .file_path("/a.rs")
                .start_line(1)
                .end_line(10)
                .language(Language::Rust)
                .build();
        storage.save_nodes(&[func], NodeLabel::Function).expect("save_nodes");

        let engine = SearchEngine::new(storage.as_ref());
        // distance = 2, so max_distance=1 should NOT match
        let results = engine
            .search_fuzzy("demo", "fethc", 1)
            .expect("fuzzy search");
        assert!(results.is_empty(), "distance 2 should not match max_distance 1");

        // max_distance=2 should match
        let results = engine
            .search_fuzzy("demo", "fethc", 2)
            .expect("fuzzy search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "fetch");
        assert_eq!(results[0].match_reason, "fuzzy d=2");
    }

    #[test]
    fn search_engine_fuzzy_rejects_empty_query() {
        let storage = build_storage();
        let engine = SearchEngine::new(storage.as_ref());
        let err = engine
            .search_fuzzy("demo", "", 2)
            .expect_err("empty query should error");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn search_engine_fuzzy_rejects_excessive_distance() {
        let storage = build_storage();
        let engine = SearchEngine::new(storage.as_ref());
        let err = engine
            .search_fuzzy("demo", "test", MAX_FUZZY_DISTANCE + 1)
            .expect_err("excessive distance should error");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn levenshtein_computes_known_distances() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("getuser", "get_user"), 1);
        assert_eq!(levenshtein("getuser", "getuser"), 0);
        assert_eq!(levenshtein("fethc", "fetch"), 2);
    }

    #[test]
    fn search_engine_graph_enhanced_filters_by_degree() {
        let storage = build_storage();
        // Create two handler functions: one with 5 incoming CALLS, one with 2.
        let handlers = [
            Node::builder(NodeLabel::Function, "request_handler", "demo.request_handler")
                .id("h1")
                .project("demo")
                .file_path("/a.rs")
                .start_line(1)
                .end_line(10)
                .language(Language::Rust)
                .build(),
            Node::builder(NodeLabel::Function, "response_handler", "demo.response_handler")
                .id("h2")
                .project("demo")
                .file_path("/a.rs")
                .start_line(20)
                .end_line(30)
                .language(Language::Rust)
                .build(),
        ];
        storage.save_nodes(&handlers, NodeLabel::Function).expect("save_nodes");

        // Create caller functions to generate CALLS edges.
        let callers: Vec<Node> = (0..7)
            .map(|i| {
                Node::builder(NodeLabel::Function, &format!("caller_{i}"), &format!("demo.caller_{i}"))
                    .id(&format!("c{i}"))
                    .project("demo")
                    .file_path("/b.rs")
                    .start_line(i + 1)
                    .end_line(i + 5)
                    .language(Language::Rust)
                    .build()
            })
            .collect();
        storage.save_nodes(&callers, NodeLabel::Function).expect("save callers");

        // 5 callers → h1, 2 callers → h2.
        let edges_to_h1: Vec<Edge> = (0..5)
            .map(|i| Edge::new(&format!("c{i}"), "h1", EdgeType::Calls, "demo"))
            .collect();
        let edges_to_h2: Vec<Edge> = (5..7)
            .map(|i| Edge::new(&format!("c{i}"), "h2", EdgeType::Calls, "demo"))
            .collect();
        let mut all_edges = edges_to_h1;
        all_edges.extend(edges_to_h2);
        storage.save_edges(&all_edges).expect("save edges");

        let engine = SearchEngine::new(storage.as_ref());
        // degree_filter=(5,100) should only return h1 (degree=5).
        let params = SearchParams {
            query: "handler".to_string(),
            mode: SearchMode::GraphEnhanced,
            degree_filter: Some((5, 100)),
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("graph search");
        assert_eq!(results.len(), 1, "only h1 should have degree >= 5");
        assert_eq!(results[0].name, "request_handler");
        assert!(results[0].match_reason.contains("degree=5"));

        // degree_filter=(0,3) should only return h2 (degree=2).
        let params2 = SearchParams {
            query: "handler".to_string(),
            mode: SearchMode::GraphEnhanced,
            degree_filter: Some((0, 3)),
            ..SearchParams::default()
        };
        let results2 = engine.search("demo", &params2).expect("graph search");
        assert_eq!(results2.len(), 1, "only h2 should have degree <= 3");
        assert_eq!(results2[0].name, "response_handler");
        assert!(results2[0].match_reason.contains("degree=2"));
    }

    #[test]
    fn search_engine_graph_enhanced_filters_by_label() {
        let storage = build_storage();
        let func = Node::builder(NodeLabel::Function, "data_handler", "demo.data_handler")
                .id("f1")
                .project("demo")
                .file_path("/a.rs")
                .start_line(1)
                .end_line(10)
                .language(Language::Rust)
                .build();
        let class = Node::builder(NodeLabel::Class, "EventHandler", "demo.EventHandler")
                .id("c1")
                .project("demo")
                .file_path("/a.rs")
                .start_line(20)
                .end_line(50)
                .language(Language::Rust)
                .build();
        storage.save_nodes(&[func], NodeLabel::Function).expect("save func");
        storage.save_nodes(&[class], NodeLabel::Class).expect("save class");

        let engine = SearchEngine::new(storage.as_ref());
        // label_filter=["Function"] → only Function nodes.
        let params = SearchParams {
            query: "handler".to_string(),
            mode: SearchMode::GraphEnhanced,
            label_filter: Some(vec!["Function".to_string()]),
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("graph search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "data_handler");
        assert_eq!(results[0].label, "Function");

        // No label filter → both Function and Class match.
        let params2 = SearchParams {
            query: "handler".to_string(),
            mode: SearchMode::GraphEnhanced,
            ..SearchParams::default()
        };
        let results2 = engine.search("demo", &params2).expect("graph search");
        assert_eq!(results2.len(), 2);
        let labels: Vec<&str> = results2.iter().map(|r| r.label.as_str()).collect();
        assert!(labels.contains(&"Function"));
        assert!(labels.contains(&"Class"));
    }

    #[test]
    fn search_engine_graph_enhanced_includes_score() {
        let storage = build_storage();
        let func = Node::builder(NodeLabel::Function, "handler", "demo.handler")
                .id("f1")
                .project("demo")
                .file_path("/a.rs")
                .start_line(1)
                .end_line(10)
                .language(Language::Rust)
                .build();
        storage.save_nodes(&[func], NodeLabel::Function).expect("save_nodes");

        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: "handler".to_string(),
            mode: SearchMode::GraphEnhanced,
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("graph search");
        assert_eq!(results.len(), 1);
        // Exact name match → score 1.0.
        assert!((results[0].score - 1.0).abs() < 1e-9);
        assert!(results[0].match_reason.contains("degree=0"));
    }

    #[test]
    fn search_engine_graph_enhanced_rejects_empty_query() {
        let storage = build_storage();
        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: String::new(),
            mode: SearchMode::GraphEnhanced,
            ..SearchParams::default()
        };
        let err = engine.search("demo", &params).expect_err("empty query should error");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn search_engine_multi_signal_exact_high_degree_same_module_with_tests() {
        // R-search-004: exact match + high degree + same module + has tests
        // → score approaches 1.0.
        let storage = build_storage();
        let handler = Node::builder(NodeLabel::Function, "handler", "demo.handler")
            .id("h1")
            .project("demo")
            .file_path("/src/handlers.rs")
            .start_line(1)
            .end_line(10)
            .language(Language::Rust)
            .build();
        storage.save_nodes(&[handler], NodeLabel::Function).expect("save_nodes");

        // 100 CALLS edges → degree_centrality = min(100/100, 1.0) = 1.0
        let calls_edges: Vec<Edge> = (0..100)
            .map(|i| Edge::new(&format!("caller_{i}"), "h1", EdgeType::Calls, "demo"))
            .collect();
        storage.save_edges(&calls_edges).expect("save calls edges");

        // 1 TESTS edge → test_coverage = 1.0
        let tests_edge = Edge::new("test_fn", "h1", EdgeType::Tests, "demo");
        storage.save_edges(&[tests_edge]).expect("save tests edge");

        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: "handler".to_string(),
            mode: SearchMode::MultiSignal,
            file_pattern: Some("/src/handlers.rs".to_string()),
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("multi-signal search");
        assert_eq!(results.len(), 1);
        let score = results[0].score;
        // name_relevance(1.0)*0.4 + degree_centrality(1.0)*0.3
        // + module_proximity(1.0)*0.2 + test_coverage(1.0)*0.1 = 1.0
        assert!(
            (score - 1.0).abs() < 1e-9,
            "exact+high_degree+same_module+tests should be 1.0, got {score}"
        );
        assert_eq!(results[0].match_reason, "multi-signal");
    }

    #[test]
    fn search_engine_multi_signal_substring_low_degree_different_module_no_tests() {
        // R-search-004: fuzzy match + low degree + different module + no tests
        // → score < 0.5.
        let storage = build_storage();
        let func = Node::builder(NodeLabel::Function, "request_handler", "demo.request_handler")
            .id("h2")
            .project("demo")
            .file_path("/src/main.rs")
            .start_line(1)
            .end_line(10)
            .language(Language::Rust)
            .build();
        storage.save_nodes(&[func], NodeLabel::Function).expect("save_nodes");

        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: "handler".to_string(),
            mode: SearchMode::MultiSignal,
            file_pattern: Some("/src/handlers.rs".to_string()),
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("multi-signal search");
        assert_eq!(results.len(), 1);
        let score = results[0].score;
        // name_relevance(0.8)*0.4 + degree_centrality(0.0)*0.3
        // + module_proximity(0.5)*0.2 + test_coverage(0.0)*0.1
        // = 0.32 + 0.0 + 0.10 + 0.0 = 0.42
        assert!(
            score < 0.5,
            "substring+low_degree+different_module+no_tests should be < 0.5, got {score}"
        );
        assert!(score >= 0.0, "score must be >= 0.0, got {score}");
    }

    #[test]
    fn search_engine_multi_signal_score_always_in_unit_range() {
        let storage = build_storage();
        let func = Node::builder(NodeLabel::Function, "handler", "demo.handler")
            .id("h1")
            .project("demo")
            .file_path("/a.rs")
            .start_line(1)
            .end_line(10)
            .language(Language::Rust)
            .build();
        storage.save_nodes(&[func], NodeLabel::Function).expect("save_nodes");

        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: "handler".to_string(),
            mode: SearchMode::MultiSignal,
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("multi-signal search");
        for r in &results {
            assert!(
                (0.0..=1.0).contains(&r.score),
                "score {} out of [0.0, 1.0]",
                r.score
            );
        }
    }

    // ===== A-04 batch: load_tested_node_ids + load_qn_to_node_id_map =====

    #[test]
    fn load_tested_node_ids_returns_all_targets_for_project() {
        let storage = build_storage();
        let targets = ["h1", "h2", "h3"];
        let edges: Vec<Edge> = targets
            .iter()
            .map(|t| Edge::new("test_fn", *t, EdgeType::Tests, "demo"))
            .collect();
        storage.save_edges(&edges).expect("save_edges");

        let engine = SearchEngine::new(storage.as_ref());
        let set = engine.load_tested_node_ids("demo").expect("load_tested_node_ids");
        assert_eq!(set.len(), 3, "expected 3 tested targets, got {set:?}");
        for t in &targets {
            assert!(set.contains(*t), "expected {t} in tested set");
        }
    }

    #[test]
    fn load_tested_node_ids_filters_by_project() {
        let storage = build_storage();
        let edges = [
            Edge::new("t1", "h1", EdgeType::Tests, "demo"),
            Edge::new("t2", "h2", EdgeType::Tests, "other"),
        ];
        storage.save_edges(&edges).expect("save_edges");

        let engine = SearchEngine::new(storage.as_ref());
        let set = engine.load_tested_node_ids("demo").expect("load_tested_node_ids");
        assert_eq!(set.len(), 1);
        assert!(set.contains("h1"));
        assert!(!set.contains("h2"));
    }

    #[test]
    fn load_tested_node_ids_returns_empty_when_no_tests_edges() {
        let storage = build_storage();
        let edges = [
            Edge::new("c1", "h1", EdgeType::Calls, "demo"),
            Edge::new("u1", "h2", EdgeType::Usage, "demo"),
        ];
        storage.save_edges(&edges).expect("save_edges");

        let engine = SearchEngine::new(storage.as_ref());
        let set = engine.load_tested_node_ids("demo").expect("load_tested_node_ids");
        assert!(set.is_empty(), "expected empty set when no TESTS edges, got {set:?}");
    }

    #[test]
    fn load_qn_to_node_id_map_returns_mapping_for_known_qns() {
        let storage = build_storage();
        let f1 = sample_function("id_f1", "demo", "foo", "demo.foo", "/a.rs", 1);
        let f2 = sample_function("id_f2", "demo", "bar", "demo.bar", "/a.rs", 10);
        storage.save_nodes(&[f1, f2], NodeLabel::Function).expect("save_nodes");

        let engine = SearchEngine::new(storage.as_ref());
        let map = engine.load_qn_to_node_id_map("demo", &["demo.foo", "demo.bar", "demo.missing"]);
        assert_eq!(map.get("demo.foo").map(String::as_str), Some("id_f1"));
        assert_eq!(map.get("demo.bar").map(String::as_str), Some("id_f2"));
        assert!(!map.contains_key("demo.missing"));
    }

    #[test]
    fn load_qn_to_node_id_map_returns_empty_for_empty_input() {
        let storage = build_storage();
        let engine = SearchEngine::new(storage.as_ref());
        let map = engine.load_qn_to_node_id_map("demo", &[]);
        assert!(map.is_empty());
    }

    #[test]
    fn load_qn_to_node_id_map_filters_by_project() {
        let storage = build_storage();
        let f1 = sample_function("id_f1", "demo", "foo", "demo.foo", "/a.rs", 1);
        let f2 = sample_function("id_f2", "other", "foo", "other.foo", "/a.rs", 1);
        storage.save_nodes(&[f1, f2], NodeLabel::Function).expect("save_nodes");

        let engine = SearchEngine::new(storage.as_ref());
        let map = engine.load_qn_to_node_id_map("demo", &["demo.foo", "other.foo"]);
        assert!(map.contains_key("demo.foo"));
        assert!(!map.contains_key("other.foo"), "other project should be filtered");
    }

    #[test]
    fn multi_signal_score_unaffected_by_storage_after_batch_load() {
        // Sanity: even when additional unrelated TESTS edges exist for other projects,
        // the score for this project's candidate is correctly computed.
        let storage = build_storage();
        let handler = Node::builder(NodeLabel::Function, "handler", "demo.handler")
            .id("h1")
            .project("demo")
            .file_path("/src/handlers.rs")
            .start_line(1)
            .end_line(10)
            .language(Language::Rust)
            .build();
        storage.save_nodes(&[handler], NodeLabel::Function).expect("save_nodes");

        let calls_edges: Vec<Edge> = (0..100)
            .map(|i| Edge::new(&format!("caller_{i}"), "h1", EdgeType::Calls, "demo"))
            .collect();
        storage.save_edges(&calls_edges).expect("save calls edges");

        // TESTS edge for h1 (demo) + noise TESTS edge for other project
        let tests_edges = [
            Edge::new("test_fn", "h1", EdgeType::Tests, "demo"),
            Edge::new("test_other", "h_other", EdgeType::Tests, "other"),
        ];
        storage.save_edges(&tests_edges).expect("save tests edges");

        let engine = SearchEngine::new(storage.as_ref());
        let params = SearchParams {
            query: "handler".to_string(),
            mode: SearchMode::MultiSignal,
            file_pattern: Some("/src/handlers.rs".to_string()),
            ..SearchParams::default()
        };
        let results = engine.search("demo", &params).expect("multi-signal search");
        assert_eq!(results.len(), 1);
        // name_relevance(1.0)*0.4 + degree_centrality(1.0)*0.3
        // + module_proximity(1.0)*0.2 + test_coverage(1.0)*0.1 = 1.0
        assert!(
            (results[0].score - 1.0).abs() < 1e-9,
            "expected score 1.0, got {}",
            results[0].score
        );
    }

    #[test]
    fn compute_name_relevance_returns_expected_values() {
        assert_eq!(compute_name_relevance("handler", "handler"), 1.0);
        assert_eq!(compute_name_relevance("HANDLER", "handler"), 1.0);
        assert_eq!(compute_name_relevance("handler_ext", "handler"), 0.8);
        assert_eq!(compute_name_relevance("my_handler", "handler"), 0.8);
        assert_eq!(compute_name_relevance("unrelated", "handler"), 0.0);
        assert_eq!(compute_name_relevance("anything", ""), 1.0);
    }


    #[test]
    fn compute_module_proximity_returns_expected_values() {
        assert_eq!(
            compute_module_proximity(&Some("/src/handlers.rs".to_string()), &Some("/src/handlers.rs".to_string())),
            1.0
        );
        assert_eq!(
            compute_module_proximity(&Some("/src/main.rs".to_string()), &Some("/src/handlers.rs".to_string())),
            0.5
        );
        assert_eq!(
            compute_module_proximity(&Some("/a.rs".to_string()), &None),
            0.5
        );
        assert_eq!(
            compute_module_proximity(&None, &Some("/a.rs".to_string())),
            0.5
        );
    }
}
