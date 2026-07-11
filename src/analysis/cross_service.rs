// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Cross-service link detection.
//!
//! Matches HTTP route patterns against string literals found in caller
//! function bodies, then persists `CROSS_SERVICE_CALLS` edges into the
//! `CodeRelation` table. v0.2.0 scope is HTTP REST only — gRPC/GraphQL/tRPC
//! detection is explicitly out of scope per the analysis spec.
//!
//! # Algorithm
//!
//! 1. Load all `Route` nodes for the project (`id`, `path`).
//! 2. Load all `Function`/`Method` nodes for the project (`id`, `filePath`,
//!    `startLine`, `content`).
//! 3. For each caller's `content`, scan for string literals using a simple
//!    `"<chars>"` extractor (no regex dependency).
//! 4. For each literal, attempt to match each route pattern in three modes:
//!    - **Exact**: literal equals pattern.
//!    - **Parameterized**: pattern contains `:id`-style segments; each
//!      `:segment` matches one non-empty, slash-free path component.
//!    - **Wildcard**: pattern contains `*`; `*` matches any sequence
//!      (including `/`).
//! 5. On match, build a [`CrossServiceLink`] and persist a
//!    `CROSS_SERVICE_CALLS` edge into the `CodeRelation` table (LadybugDB
//!    stores edges as nodes per the convention used by `community.rs`).
//!
//! # Deterministic matching
//!
//! Per Rule 5 (确定性逻辑禁止交给模型), route matching is implemented with
//! explicit string segmentation — no regex engine is invoked. The match
//! outcome is fully determined by the pattern and literal bytes.

use crate::storage::capability::Storage;
use crate::storage::error::Result as StorageResult;
use crate::storage::schema::escape_cypher_string;
use serde::Serialize;

/// Edge type stored on `CodeRelation.type` for cross-service links.
const CROSS_SERVICE_CALLS_EDGE_TYPE: &str = "CROSS_SERVICE_CALLS";

/// The kind of match between a route pattern and a string literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MatchType {
    /// Pattern and literal are byte-equal.
    Exact,
    /// Pattern contains `:param` segments; literal matches the structure.
    Parameterized,
    /// Pattern contains `*` wildcards; literal matches the glob.
    Wildcard,
}

/// A single cross-service link between a route and a caller.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CrossServiceLink {
    /// The `Route` node id.
    pub route_id: String,
    /// The route pattern (e.g. `/api/users/:id`).
    pub route_pattern: String,
    /// The caller `Function`/`Method` node id.
    pub caller_id: String,
    /// The caller source file path.
    pub caller_file: String,
    /// The caller start line (1-based).
    pub caller_line: u32,
    /// How the route pattern matched the caller's string literal.
    pub match_type: MatchType,
}

/// Detects cross-service links by matching route patterns against caller
/// string literals.
///
/// Backed by a `&'a dyn Storage` capability, matching the convention used by
/// [`crate::analysis::architecture::ArchitectureAnalyzer`] and
/// [`crate::analysis::community::CommunityDetector`]. The `project` field is
/// captured at construction so [`link`] can scope every Cypher query via
/// `WHERE ... = $project` (multi-project isolation rule).
///
/// [`link`]: CrossServiceLinker::link
pub struct CrossServiceLinker<'a> {
    storage: &'a dyn Storage,
    project: String,
}

impl<'a> CrossServiceLinker<'a> {
    /// Creates a new linker for `project` backed by the given storage
    /// capability.
    #[must_use]
    pub fn new(storage: &'a dyn Storage, project: impl Into<String>) -> Self {
        Self {
            storage,
            project: project.into(),
        }
    }

    /// Detects cross-service links and persists `CROSS_SERVICE_CALLS` edges.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if any underlying Cypher query or write
    /// fails.
    pub fn link(&self) -> StorageResult<Vec<CrossServiceLink>> {
        let routes = self.load_routes()?;
        if routes.is_empty() {
            return Ok(Vec::new());
        }
        let callers = self.load_callers()?;
        if callers.is_empty() {
            return Ok(Vec::new());
        }

        let existing_edges = self.load_existing_edges()?;
        let mut links: Vec<CrossServiceLink> = Vec::new();
        let mut next_edge_id = self.next_edge_id(&existing_edges)?;

        for caller in &callers {
            if caller.content.is_empty() {
                continue;
            }
            let literals = extract_string_literals(&caller.content);
            for literal in &literals {
                for route in &routes {
                    if let Some(match_type) = match_route(&route.path, literal) {
                        let edge_key = (caller.id.clone(), route.id.clone());
                        if !existing_edges.contains(&edge_key) {
                            self.persist_edge(&next_edge_id, &caller.id, &route.id)?;
                            next_edge_id += 1;
                        }
                        links.push(CrossServiceLink {
                            route_id: route.id.clone(),
                            route_pattern: route.path.clone(),
                            caller_id: caller.id.clone(),
                            caller_file: caller.file_path.clone(),
                            caller_line: caller.start_line,
                            match_type,
                        });
                    }
                }
            }
        }
        Ok(links)
    }

    /// Loads all `Route` nodes for the project (`id`, `path`).
    fn load_routes(&self) -> StorageResult<Vec<RouteRow>> {
        let escaped = escape_cypher_string(&self.project);
        let cypher = format!(
            "MATCH (r:Route) WHERE r.project = '{escaped}' \
             RETURN r.id AS id, r.path AS path;"
        );
        let rows = self.storage.query(&cypher)?;
        let mut out = Vec::new();
        for row in rows {
            if row.len() < 2 {
                continue;
            }
            let id = row[0].as_str().unwrap_or_default().to_string();
            let path = row[1].as_str().unwrap_or_default().to_string();
            if !id.is_empty() && !path.is_empty() {
                out.push(RouteRow { id, path });
            }
        }
        Ok(out)
    }

    /// Loads all `Function` and `Method` nodes for the project with their
    /// `content` field. LadybugDB Cypher subset does not support `UNION`,
    /// so we issue two queries and merge in Rust (same pattern as
    /// `architecture.rs`).
    fn load_callers(&self) -> StorageResult<Vec<CallerRow>> {
        let escaped = escape_cypher_string(&self.project);
        let mut out = Vec::new();
        for table in &["Function", "Method"] {
            let cypher = format!(
                "MATCH (n:{table}) WHERE n.project = '{escaped}' \
                 RETURN n.id AS id, n.filePath AS file_path, \
                 n.startLine AS start_line, n.content AS content;"
            );
            let rows = self.storage.query(&cypher)?;
            for row in rows {
                if row.len() < 4 {
                    continue;
                }
                let id = row[0].as_str().unwrap_or_default().to_string();
                let file_path = row[1].as_str().unwrap_or_default().to_string();
                let start_line = row[2]
                    .as_i64()
                    .map(|v| v as u32)
                    .or_else(|| row[2].as_u64().map(|v| v as u32))
                    .unwrap_or(0);
                let content = row[3].as_str().unwrap_or_default().to_string();
                if !id.is_empty() {
                    out.push(CallerRow {
                        id,
                        file_path,
                        start_line,
                        content,
                    });
                }
            }
        }
        Ok(out)
    }

    /// Loads existing `CROSS_SERVICE_CALLS` edges for the project, returning
    /// the set of `(caller_id, route_id)` pairs already persisted. Used for
    /// idempotent insertion (no duplicate edges on repeated `link()` calls).
    fn load_existing_edges(&self) -> StorageResult<std::collections::HashSet<(String, String)>> {
        let escaped = escape_cypher_string(&self.project);
        let cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type = 'CROSS_SERVICE_CALLS' \
             AND e.project = '{escaped}' \
             RETURN e.source AS source, e.target AS target;"
        );
        let rows = self.storage.query(&cypher)?;
        let mut out = std::collections::HashSet::new();
        for row in rows {
            if row.len() < 2 {
                continue;
            }
            let src = row[0].as_str().unwrap_or_default().to_string();
            let dst = row[1].as_str().unwrap_or_default().to_string();
            if !src.is_empty() && !dst.is_empty() {
                out.insert((src, dst));
            }
        }
        Ok(out)
    }

    /// Returns the next edge id to use for new `CROSS_SERVICE_CALLS` edges.
    /// Uses a deterministic prefix + 1-based index to keep ids readable.
    fn next_edge_id(
        &self,
        existing: &std::collections::HashSet<(String, String)>,
    ) -> StorageResult<u64> {
        let escaped = escape_cypher_string(&self.project);
        let cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type = 'CROSS_SERVICE_CALLS' \
             AND e.project = '{escaped}' RETURN e.id AS id;"
        );
        let rows = self.storage.query(&cypher)?;
        let mut max_idx = 0u64;
        for row in rows {
            if let Some(id_str) = row.first().and_then(|v| v.as_str()) {
                if let Some(suffix) = id_str.strip_prefix("csl_") {
                    if let Ok(n) = suffix.parse::<u64>() {
                        if n > max_idx {
                            max_idx = n;
                        }
                    }
                }
            }
        }
        let _ = existing; // mark used (existing edges consulted separately)
        Ok(max_idx + 1)
    }

    /// Persists a single `CROSS_SERVICE_CALLS` edge into the `CodeRelation`
    /// table. Source = caller id, target = route id (matches the
    /// `HANDLES` edge direction convention used by `architecture.rs`).
    fn persist_edge(&self, idx: &u64, caller_id: &str, route_id: &str) -> StorageResult<()> {
        let edge_id = format!("csl_{idx}");
        let cypher = format!(
            "CREATE (:CodeRelation {{id: '{}', source: '{}', target: '{}', \
             type: '{}', confidence: 1.0, confidenceTier: 'High', reason: 'route pattern match', \
             startLine: 0, project: '{}'}});",
            escape_cypher_string(&edge_id),
            escape_cypher_string(caller_id),
            escape_cypher_string(route_id),
            CROSS_SERVICE_CALLS_EDGE_TYPE,
            escape_cypher_string(&self.project),
        );
        self.storage.execute(&cypher)
    }
}

/// Internal row representation for a `Route` node.
struct RouteRow {
    id: String,
    path: String,
}

/// Internal row representation for a caller `Function`/`Method` node.
struct CallerRow {
    id: String,
    file_path: String,
    start_line: u32,
    content: String,
}

// ---------------------------------------------------------------------------
// Route pattern matching (deterministic, no regex — Rule 5)
// ---------------------------------------------------------------------------

/// Attempts to match `pattern` against `literal`, returning the match type.
///
/// Precedence: Exact > Parameterized > Wildcard. Returns `None` if no mode
/// matches.
#[must_use]
fn match_route(pattern: &str, literal: &str) -> Option<MatchType> {
    if pattern == literal {
        return Some(MatchType::Exact);
    }
    if pattern.contains(':') && match_parameterized(pattern, literal) {
        return Some(MatchType::Parameterized);
    }
    if pattern.contains('*') && match_wildcard(pattern, literal) {
        return Some(MatchType::Wildcard);
    }
    None
}

/// Parameterized matching: split pattern and literal on `/`, each `:seg`
/// matches one non-empty slash-free component, other segments must match
/// exactly.
fn match_parameterized(pattern: &str, literal: &str) -> bool {
    let pat_parts: Vec<&str> = pattern.split('/').collect();
    let lit_parts: Vec<&str> = literal.split('/').collect();
    if pat_parts.len() != lit_parts.len() {
        return false;
    }
    for (p, l) in pat_parts.iter().zip(lit_parts.iter()) {
        if p.starts_with(':') && !p.is_empty() {
            if l.is_empty() {
                return false;
            }
        } else if *p != *l {
            return false;
        }
    }
    true
}

/// Wildcard matching: `*` matches any sequence (including `/`). Multiple
/// `*` are handled by splitting the pattern on `*` and requiring the
/// literal to contain the literal parts in order, anchored at the start of
/// the first part and the end of the last part.
fn match_wildcard(pattern: &str, literal: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == literal;
    }
    let first = parts[0];
    if !literal.starts_with(first) {
        return false;
    }
    let mut cursor = first.len();
    for part in &parts[1..parts.len() - 1] {
        if part.is_empty() {
            continue;
        }
        let rest = &literal[cursor..];
        match rest.find(part) {
            Some(idx) => cursor += idx + part.len(),
            None => return false,
        }
    }
    let last = parts.last().copied().unwrap_or("");
    if !last.is_empty() && !literal[cursor..].ends_with(last) {
        return false;
    }
    true
}

/// Extracts double-quoted string literals from `content`. Handles simple
/// escape sequences (`\"`, `\\`) naively — sufficient for route strings,
/// which rarely contain quotes.
fn extract_string_literals(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let start = i + 1;
            let mut j = start;
            let mut buf = String::new();
            while j < bytes.len() {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    match bytes[j + 1] {
                        b'"' => {
                            buf.push('"');
                            j += 2;
                            continue;
                        }
                        b'\\' => {
                            buf.push('\\');
                            j += 2;
                            continue;
                        }
                        b'n' => {
                            buf.push('\n');
                            j += 2;
                            continue;
                        }
                        b't' => {
                            buf.push('\t');
                            j += 2;
                            continue;
                        }
                        _ => {
                            buf.push('\\');
                            j += 1;
                            continue;
                        }
                    }
                }
                if bytes[j] == b'"' {
                    break;
                }
                buf.push(bytes[j] as char);
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'"' {
                out.push(buf);
                i = j + 1;
            } else {
                i = start;
            }
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, StorageModule};
    use tempfile::TempDir;

    // --- Test helpers (mirror community.rs pattern) ---

    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cross_service_testdb");
        std::mem::forget(dir);
        path
    }

    fn build_kit_for_db(db: &std::path::Path) -> Kit {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        build_kit(&config).expect("build_kit")
    }

    fn storage(kit: &AsyncKit<AsyncReady>) -> std::sync::Arc<dyn crate::storage::capability::Storage> {
        kit.require::<StorageModule>().expect("require_storage")
    }

    fn create_route(kit: &AsyncKit<AsyncReady>, id: &str, project: &str, path: &str) {
        let s = storage(kit);
        let cypher = format!(
            "CREATE (:Route {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '{}', parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(path),
            escape_cypher_string(path),
            escape_cypher_string(path),
        );
        s.execute(&cypher).expect("create route");
    }

    fn create_function_with_content(
        kit: &AsyncKit<AsyncReady>,
        id: &str,
        project: &str,
        name: &str,
        qn: &str,
        file: &str,
        line: u32,
        content: &str,
    ) {
        let s = storage(kit);
        let end_line = line + 10;
        let cypher = format!(
            "CREATE (:Function {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '{}', startLine: {}, endLine: {}, signature: '', returnType: '', \
             isExported: false, docstring: '', content: '{}', parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(name),
            escape_cypher_string(qn),
            escape_cypher_string(file),
            line,
            end_line,
            escape_cypher_string(content),
        );
        s.execute(&cypher).expect("create function");
    }

    fn count_cross_service_edges(kit: &AsyncKit<AsyncReady>, project: &str) -> usize {
        let s = storage(kit);
        let cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type = 'CROSS_SERVICE_CALLS' AND e.project = '{}' \
             RETURN e.id AS id;",
            escape_cypher_string(project),
        );
        let rows = s.query(&cypher).expect("query cross-service edges");
        rows.len()
    }

    // ====================================================================
    // R-analysis-005: route pattern matching unit tests
    // ====================================================================

    #[test]
    fn match_route_exact() {
        assert_eq!(
            match_route("/api/users", "/api/users"),
            Some(MatchType::Exact)
        );
    }

    #[test]
    fn match_route_parameterized_basic() {
        assert_eq!(
            match_route("/api/users/:id", "/api/users/123"),
            Some(MatchType::Parameterized)
        );
        assert_eq!(
            match_route("/api/users/:id", "/api/users/456"),
            Some(MatchType::Parameterized)
        );
    }

    #[test]
    fn match_route_parameterized_rejects_empty_segment() {
        assert_eq!(
            match_route("/api/users/:id", "/api/users/"),
            None,
            "empty parameter segment should not match"
        );
    }

    #[test]
    fn match_route_parameterized_rejects_extra_segment() {
        assert_eq!(
            match_route("/api/users/:id", "/api/users/123/extra"),
            None,
            "extra segment should not match"
        );
    }

    #[test]
    fn match_route_wildcard_basic() {
        assert_eq!(
            match_route("/api/*", "/api/anything"),
            Some(MatchType::Wildcard)
        );
        assert_eq!(
            match_route("/api/*", "/api/foo/bar"),
            Some(MatchType::Wildcard)
        );
    }

    #[test]
    fn match_route_wildcard_middle() {
        assert_eq!(
            match_route("/api/*/users", "/api/v2/users"),
            Some(MatchType::Wildcard)
        );
    }

    #[test]
    fn match_route_no_match_for_unrelated() {
        assert_eq!(match_route("/api/users", "/api/products"), None);
        assert_eq!(match_route("/api/users/:id", "/products/123"), None);
        assert_eq!(match_route("/api/*", "/products/foo"), None);
    }

    #[test]
    fn match_route_exact_takes_precedence_over_parameterized() {
        // Pattern without `:` should match exactly, not parameterized.
        assert_eq!(
            match_route("/api/users", "/api/users"),
            Some(MatchType::Exact)
        );
    }

    #[test]
    fn extract_string_literals_basic() {
        let content = r#"let url = "/api/users"; fetch("/api/users/123");"#;
        let lits = extract_string_literals(content);
        assert_eq!(
            lits,
            vec!["/api/users".to_string(), "/api/users/123".to_string()]
        );
    }

    #[test]
    fn extract_string_literals_handles_escaped_quote() {
        let content = r#"let s = "he said \"hi\"";"#;
        let lits = extract_string_literals(content);
        assert_eq!(lits, vec!["he said \"hi\"".to_string()]);
    }

    #[test]
    fn extract_string_literals_empty_when_no_quotes() {
        let lits = extract_string_literals("fn main() { return 42; }");
        assert!(lits.is_empty());
    }

    #[test]
    fn extract_string_literals_skips_unterminated() {
        let lits = extract_string_literals("let s = \"unterminated;");
        assert!(
            lits.is_empty(),
            "unterminated string should yield no literal"
        );
    }

    // ====================================================================
    // R-analysis-005: CrossServiceLinker::link storage integration
    // ====================================================================

    #[test]
    fn link_returns_empty_for_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        let linker = CrossServiceLinker::new(&*s, "demo");
        let result = linker.link().expect("link");
        assert!(result.is_empty(), "empty DB → empty links");
    }

    #[test]
    fn link_matches_exact_route() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_route(&kit, "r1", "demo", "/api/users");
        create_function_with_content(
            &kit,
            "f1",
            "demo",
            "caller",
            "demo.caller",
            "/src/caller.rs",
            10,
            r#"let url = "/api/users"; fetch(url);"#,
        );

        let s = storage(&kit);
        let linker = CrossServiceLinker::new(&*s, "demo");
        let links = linker.link().expect("link");
        assert_eq!(links.len(), 1, "should match 1 exact route");
        assert_eq!(links[0].route_id, "r1");
        assert_eq!(links[0].route_pattern, "/api/users");
        assert_eq!(links[0].caller_id, "f1");
        assert_eq!(links[0].caller_file, "/src/caller.rs");
        assert_eq!(links[0].caller_line, 10);
        assert_eq!(links[0].match_type, MatchType::Exact);
    }

    #[test]
    fn link_matches_parameterized_route() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_route(&kit, "r1", "demo", "/api/users/:id");
        create_function_with_content(
            &kit,
            "f1",
            "demo",
            "caller",
            "demo.caller",
            "/src/caller.rs",
            5,
            r#"let url = "/api/users/123";"#,
        );

        let s = storage(&kit);
        let linker = CrossServiceLinker::new(&*s, "demo");
        let links = linker.link().expect("link");
        assert_eq!(links.len(), 1, "should match 1 parameterized route");
        assert_eq!(links[0].match_type, MatchType::Parameterized);
    }

    #[test]
    fn link_matches_wildcard_route() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_route(&kit, "r1", "demo", "/api/*");
        create_function_with_content(
            &kit,
            "f1",
            "demo",
            "caller",
            "demo.caller",
            "/src/caller.rs",
            7,
            r#"let url = "/api/anything";"#,
        );

        let s = storage(&kit);
        let linker = CrossServiceLinker::new(&*s, "demo");
        let links = linker.link().expect("link");
        assert_eq!(links.len(), 1, "should match 1 wildcard route");
        assert_eq!(links[0].match_type, MatchType::Wildcard);
    }

    #[test]
    fn link_no_match_for_unrelated_route() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_route(&kit, "r1", "demo", "/api/users");
        create_function_with_content(
            &kit,
            "f1",
            "demo",
            "caller",
            "demo.caller",
            "/src/caller.rs",
            1,
            r#"let url = "/api/products";"#,
        );

        let s = storage(&kit);
        let linker = CrossServiceLinker::new(&*s, "demo");
        let links = linker.link().expect("link");
        assert!(links.is_empty(), "unrelated literal should not match");
    }

    #[test]
    fn link_persists_cross_service_calls_edges() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_route(&kit, "r1", "demo", "/api/users");
        create_function_with_content(
            &kit,
            "f1",
            "demo",
            "caller",
            "demo.caller",
            "/src/caller.rs",
            10,
            r#"fetch("/api/users");"#,
        );

        let s = storage(&kit);
        let linker = CrossServiceLinker::new(&*s, "demo");
        let links = linker.link().expect("link");
        assert_eq!(links.len(), 1);
        // Edge should be persisted in CodeRelation table.
        assert_eq!(
            count_cross_service_edges(&kit, "demo"),
            1,
            "CROSS_SERVICE_CALLS edge should be persisted"
        );
    }

    #[test]
    fn link_skips_empty_content_function() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_route(&kit, "r1", "demo", "/api/users");
        create_function_with_content(
            &kit,
            "f1",
            "demo",
            "caller",
            "demo.caller",
            "/src/caller.rs",
            1,
            "", // empty content
        );

        let s = storage(&kit);
        let linker = CrossServiceLinker::new(&*s, "demo");
        let links = linker.link().expect("link");
        assert!(links.is_empty(), "empty content → no links");
    }

    #[test]
    fn link_filters_by_project() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_route(&kit, "r1", "demo", "/api/users");
        create_route(&kit, "r2", "other", "/api/users");
        create_function_with_content(
            &kit,
            "f1",
            "demo",
            "caller",
            "demo.caller",
            "/src/caller.rs",
            1,
            r#"fetch("/api/users");"#,
        );

        let s = storage(&kit);
        let linker = CrossServiceLinker::new(&*s, "demo");
        let links = linker.link().expect("link");
        assert_eq!(links.len(), 1, "should only match demo's route");
        assert_eq!(links[0].route_id, "r1");
    }

    #[test]
    fn link_matches_multiple_literals() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_route(&kit, "r1", "demo", "/api/users");
        create_route(&kit, "r2", "demo", "/api/products");
        create_function_with_content(
            &kit,
            "f1",
            "demo",
            "caller",
            "demo.caller",
            "/src/caller.rs",
            1,
            r#"let a = "/api/users"; let b = "/api/products";"#,
        );

        let s = storage(&kit);
        let linker = CrossServiceLinker::new(&*s, "demo");
        let links = linker.link().expect("link");
        assert_eq!(links.len(), 2, "should match 2 routes");
        let patterns: Vec<&str> = links.iter().map(|l| l.route_pattern.as_str()).collect();
        assert!(patterns.contains(&"/api/users"));
        assert!(patterns.contains(&"/api/products"));
    }

    #[test]
    fn link_idempotent_no_duplicate_edges() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_route(&kit, "r1", "demo", "/api/users");
        create_function_with_content(
            &kit,
            "f1",
            "demo",
            "caller",
            "demo.caller",
            "/src/caller.rs",
            1,
            r#"fetch("/api/users");"#,
        );

        let s = storage(&kit);
        // First call → 1 link, 1 edge.
        let links1 = CrossServiceLinker::new(&*s, "demo").link().expect("link");
        assert_eq!(links1.len(), 1);
        assert_eq!(count_cross_service_edges(&kit, "demo"), 1);
        // Second call → should not double-insert edges for the same
        // (route, caller) pair. We accept either: (a) skip duplicates, or
        // (b) the second call returns 1 link but no new edge. Either way,
        // total edges after 2 calls should be 1 (not 2).
        let links2 = CrossServiceLinker::new(&*s, "demo").link().expect("link");
        assert_eq!(links2.len(), 1, "second call should still detect the link");
        assert_eq!(
            count_cross_service_edges(&kit, "demo"),
            1,
            "idempotent: no duplicate edges"
        );
    }

    // ====================================================================
    // Early-return paths in CrossServiceLinker::link
    // ====================================================================

    #[test]
    fn link_returns_empty_when_routes_exist_but_no_callers() {
        // Routes exist, but no Function/Method nodes → callers empty → early
        // return with empty Vec (line 109).
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_route(&kit, "r1", "demo", "/api/users");
        // Deliberately create no functions/methods.

        let s = storage(&kit);
        let linker = CrossServiceLinker::new(&*s, "demo");
        let links = linker.link().expect("link");
        assert!(links.is_empty(), "routes but no callers → empty links");
    }

    // ====================================================================
    // match_parameterized: static-segment mismatch
    // ====================================================================

    #[test]
    fn match_parameterized_returns_false_when_static_segment_differs() {
        // Pattern has a `:param` but a non-parameter segment doesn't match.
        // Covers the `else if *p != *l { return false; }` branch.
        assert!(
            !match_parameterized("/api/users/:id", "/api/products/123"),
            "mismatched static segment should not match"
        );
        assert!(
            !match_parameterized("/foo/:id/bar", "/foo/123/baz"),
            "mismatched trailing static segment should not match"
        );
    }

    // ====================================================================
    // match_wildcard: multi-star patterns
    // ====================================================================

    #[test]
    fn match_wildcard_skips_empty_middle_part_from_double_star() {
        // `**` splits to ["", ""] as middle parts — the empty middle part
        // should be skipped via `continue` (line 350-351).
        assert!(
            match_wildcard("/api/**/end", "/api/foo/bar/end"),
            "double-star should match any path between /api/ and /end"
        );
    }

    #[test]
    fn match_wildcard_finds_middle_literal_between_two_stars() {
        // Two wildcards with literal content between them — exercises the
        // `rest.find(part)` match arm (lines 353-356).
        assert!(
            match_wildcard("/api/*/x/*/end", "/api/v1/x/data/end"),
            "should find '/x/' between wildcards"
        );
        // Middle literal not found → None arm (line 356).
        assert!(
            !match_wildcard("/api/*/x/*/end", "/api/v1/y/data/end"),
            "should fail when middle literal '/x/' is absent"
        );
    }

    #[test]
    fn match_wildcard_rejects_when_last_part_is_not_suffix() {
        // Last segment of pattern doesn't match the suffix of literal.
        // Covers `return false` for non-empty last not matching ends_with.
        assert!(
            !match_wildcard("/api/*/users", "/api/v2/products"),
            "should fail when literal doesn't end with '/users'"
        );
    }

    // ====================================================================
    // extract_string_literals: escape-sequence coverage
    // ====================================================================

    #[test]
    fn extract_string_literals_decodes_escaped_backslash() {
        // `\\` in source → single `\` in extracted literal (lines 387-389).
        let content = r#"let s = "path\\to";"#;
        let lits = extract_string_literals(content);
        assert_eq!(lits, vec![r"path\to".to_string()]);
    }

    #[test]
    fn extract_string_literals_decodes_escaped_newline() {
        // `\n` in source → actual newline char (lines 392-394).
        let content = r#"let s = "line1\nline2";"#;
        let lits = extract_string_literals(content);
        assert_eq!(lits, vec!["line1\nline2".to_string()]);
    }

    #[test]
    fn extract_string_literals_decodes_escaped_tab() {
        // `\t` in source → actual tab char (lines 397-399).
        let content = r#"let s = "col1\tcol2";"#;
        let lits = extract_string_literals(content);
        assert_eq!(lits, vec!["col1\tcol2".to_string()]);
    }

    #[test]
    fn extract_string_literals_preserves_backslash_for_unknown_escape() {
        // Unknown escape `\y` → backslash is kept, `y` processed normally
        // (lines 402-404, the `_` match arm).
        let content = r#"let s = "x\y";"#;
        let lits = extract_string_literals(content);
        assert_eq!(lits, vec![r"x\y".to_string()]);
    }

    // ====================================================================
    // match_wildcard: no-star pattern direct invocation
    // ====================================================================

    #[test]
    fn match_wildcard_no_star_pattern_compares_literal_equality() {
        // match_wildcard is normally only called from match_route when the
        // pattern contains `*`. Calling it directly with a no-star pattern
        // hits the `parts.len() == 1` early return (line 342), which
        // reduces to byte-equality comparison.
        assert!(
            match_wildcard("/api/users", "/api/users"),
            "no-star pattern equal to literal → true"
        );
        assert!(
            !match_wildcard("/api/users", "/api/products"),
            "no-star pattern different from literal → false"
        );
        assert!(
            match_wildcard("", ""),
            "empty no-star pattern matches empty literal"
        );
    }

    // ====================================================================
    // load_routes / load_callers / load_existing_edges: defensive row-shape
    // guards. LadybugDB always returns the requested number of columns
    // (NULL for missing properties), so row.len() < N is never true with
    // well-formed Cypher RETURN clauses. These tests verify the happy path
    // for nodes created with minimal property sets, confirming the guards
    // are defensive (not reachable through normal storage behavior).
    // ====================================================================

    #[test]
    fn link_handles_route_node_with_null_optional_properties() {
        // Route node created with only the required id/project/path fields.
        // Missing properties (name, qualifiedName, etc.) are NULL in the DB,
        // but load_routes only reads id and path → row has 2 elements →
        // the row.len() < 2 guard is not triggered (it's a defensive check).
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        // Create a minimal Route node with only id, project, and path.
        let cypher = format!("CREATE (:Route {{id: 'r1', project: 'demo', path: '/api/users'}});");
        s.execute(&cypher).expect("create minimal route");
        create_function_with_content(
            &kit,
            "f1",
            "demo",
            "caller",
            "demo.caller",
            "/src/caller.rs",
            1,
            r#"fetch("/api/users");"#,
        );

        let linker = CrossServiceLinker::new(&*s, "demo");
        let links = linker.link().expect("link");
        assert_eq!(links.len(), 1, "minimal Route node should still match");
        assert_eq!(links[0].route_id, "r1");
        assert_eq!(links[0].match_type, MatchType::Exact);
    }

    #[test]
    fn link_handles_caller_node_with_null_optional_properties() {
        // Function node created with only the fields load_callers reads
        // (id, filePath, startLine, content). Missing properties are NULL
        // but the row still has 4 elements → row.len() < 4 guard is not
        // triggered (defensive only).
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let s = storage(&kit);
        create_route(&kit, "r1", "demo", "/api/users");
        // Create a minimal Function node with only the fields load_callers
        // reads. Other required columns are omitted (NULL in DB).
        let cypher = format!(
            "CREATE (:Function {{id: 'f1', project: 'demo', filePath: '/src/caller.rs', \
             startLine: 5, content: 'fetch(\"/api/users\")'}});"
        );
        s.execute(&cypher).expect("create minimal function");

        let linker = CrossServiceLinker::new(&*s, "demo");
        let links = linker.link().expect("link");
        assert_eq!(links.len(), 1, "minimal Function node should still match");
        assert_eq!(links[0].caller_id, "f1");
        assert_eq!(links[0].caller_line, 5);
    }
}
