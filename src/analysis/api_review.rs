// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! API review toolkit.
//!
//! Provides four graph-based analysis capabilities over the existing
//! LadybugDB graph:
//! - [`ApiReviewer::route_map`] — lists all `Route`/`Endpoint` nodes joined
//!   with their `Handler` and `Middleware` via `HANDLES`/`USES` edges.
//! - [`ApiReviewer::shape_check`] — validates API endpoint schema consistency
//!   by comparing an `Endpoint` node's `expectedSchema` property with the
//!   actual schema recorded on `CALLS` edges pointing to it.
//! - [`ApiReviewer::api_impact`] — traces which callers would be affected by
//!   changing an endpoint, by reverse-traversing `CALLS` edges from the
//!   endpoint's handler.
//! - [`ApiReviewer::tool_map`] — lists all `Tool` nodes (MCP tools) joined
//!   with their handler functions via `HANDLES` edges.
//!
//! # Algorithm
//!
//! Each method issues Cypher queries against the `&dyn Storage` capability
//! and joins results in Rust. LadybugDB's Cypher subset does not support
//! `GROUP BY`, multi-label `WHERE (n:A OR n:B)` expressions, or `UNION`, so
//! we issue separate queries per node label and aggregate in Rust — matching
//! the pattern established by [`crate::analysis::architecture`] and
//! [`crate::analysis::dead_code`].
//!
//! Edges are stored as `CodeRelation` nodes (not true graph edges) with
//! `source`/`target`/`type` fields. The `HANDLES` edge direction is
//! `source = handler_id → target = route/tool/endpoint_id`. The `CALLS` edge
//! direction is `source = caller_id → target = callee_id`.

use crate::storage::capability::Storage;
use crate::storage::error::Result as StorageResult;
use crate::storage::schema::escape_cypher_string;
use serde::Serialize;

/// Severity label recorded on [`ShapeViolation`] when expected and actual
/// schemas differ.
const SEVERITY_MISMATCH: &str = "mismatch";

/// A single route-map entry: one route + its handler + middleware chain.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RouteEntry {
    /// Route path (e.g. `/api/users`).
    pub path: String,
    /// HTTP method (e.g. `GET`, `POST`). Empty if not set.
    pub method: String,
    /// Handler node id. Empty if no handler linked.
    pub handler_id: String,
    /// Handler function name. Empty if no handler linked.
    pub handler_name: String,
    /// Middleware names wrapping the handler (in edge order).
    pub middleware: Vec<String>,
}

/// A schema mismatch between an endpoint's expected schema and the actual
/// schema recorded on a call edge.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ShapeViolation {
    /// Endpoint path (e.g. `/api/users`).
    pub endpoint: String,
    /// Expected schema (JSON string from `Endpoint.expectedSchema`).
    pub expected_schema: String,
    /// Actual schema (JSON string from `CodeRelation.reason`).
    pub actual_schema: String,
    /// Always `SEVERITY_MISMATCH` when expected != actual.
    pub severity: String,
}

/// A single impact entry: one caller affected by an endpoint change.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ImpactEntry {
    /// Endpoint path that was analysed.
    pub endpoint: String,
    /// Caller function name.
    pub affected_caller: String,
    /// Caller source file path.
    pub caller_file: String,
    /// Caller 1-based start line.
    pub caller_line: u32,
}

/// A single tool-map entry: one MCP tool + its handler function.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ToolEntry {
    /// Tool node name (e.g. `query`).
    pub tool_name: String,
    /// Handler node id. Empty if no handler linked.
    pub handler_id: String,
    /// Handler function name. Empty if no handler linked.
    pub handler_name: String,
}

/// Provides API review analysis for a project.
///
/// All four methods take a `&dyn Storage` (the trait-kit capability) rather
/// than a `&StorageConnection` directly, matching the codebase convention
/// used by [`crate::analysis::dead_code::DeadCodeDetector`] and
/// [`crate::analysis::architecture::ArchitectureAnalyzer`].
pub struct ApiReviewer<'a> {
    storage: &'a dyn Storage,
}

impl<'a> ApiReviewer<'a> {
    /// Creates a new reviewer backed by the given storage capability.
    #[must_use]
    pub fn new(storage: &'a dyn Storage) -> Self {
        Self { storage }
    }

    /// Generates a route map: all `Route`/`Endpoint` nodes joined with their
    /// handler functions and middleware.
    ///
    /// # Errors
    ///
    /// Returns [`crate::storage::error::StorageError`] if any Cypher query
    /// fails.
    pub fn route_map(&self, project: &str) -> StorageResult<Vec<RouteEntry>> {
        // (a) Load all Route + Endpoint nodes (same schema, separate queries).
        let routes = self.load_routes(project)?;

        // (b) Load all Handler nodes (id → name).
        let handlers = self.load_handlers(project)?;

        // (c) Load all Middleware nodes (id → name).
        let middleware = self.load_middleware(project)?;

        // (d) Load HANDLES edges (source = handler id, target = route id).
        let handles = self.load_edges(project, "HANDLES")?;

        // (e) Load USES edges (source = middleware id, target = handler id).
        let uses = self.load_edges(project, "USES")?;

        // (f) Build handler → middleware list map.
        // Single-line for coverage: tarpaulin attribute continuation
        let mut handler_to_mw: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for (mw_id, h_id) in &uses {
            if let Some(mw_name) = middleware.get(mw_id) {
                handler_to_mw
                    .entry(h_id.clone())
                    .or_default()
                    .push(mw_name.clone());
            }
        }

        // (g) Build route → handler map.
        let mut route_to_handler: std::collections::HashMap<String, (String, String)> =
            std::collections::HashMap::new(); // Single-line for coverage: tarpaulin attribute continuation
        for (h_id, r_id) in &handles {
            if let Some(h_name) = handlers.get(h_id) {
                route_to_handler
                    .entry(r_id.clone())
                    .or_insert_with(|| (h_id.clone(), h_name.clone()));
            }
        }

        // (h) Build result: for each route, look up handler + middleware.
        let mut result: Vec<RouteEntry> = routes
            .into_iter()
            .map(|(id, path, method)| {
                let (handler_id, handler_name) =
                    route_to_handler.get(&id).cloned().unwrap_or_default();
                let mw = handler_to_mw.get(&handler_id).cloned().unwrap_or_default();
                RouteEntry {
                    path,
                    method,
                    handler_id,
                    handler_name,
                    middleware: mw,
                }
            })
            .collect();
        // Sort by path for determinism.
        result.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(result)
    }

    /// Checks API shape consistency: compares each `Endpoint` node's
    /// `expectedSchema` property with the actual schema recorded on `CALLS`
    /// edges pointing to it.
    ///
    /// # Errors
    ///
    /// Returns [`crate::storage::error::StorageError`] if any Cypher query
    /// fails.
    pub fn shape_check(&self, project: &str) -> StorageResult<Vec<ShapeViolation>> {
        // (a) Load all Endpoint nodes with expectedSchema.
        let endpoints = self.load_endpoints_with_schema(project)?;

        // (b) Load CALLS edges pointing to endpoints.
        let calls = self.load_edges(project, "CALLS")?;

        // (c) For each endpoint with a non-empty expectedSchema, find CALLS
        //     edges pointing to it and compare schemas.
        let mut result = Vec::new();
        for (ep_id, ep_path, expected_schema) in &endpoints {
            if expected_schema.is_empty() {
                continue;
            }
            for (_caller_id, callee_id) in &calls {
                if callee_id != ep_id {
                    continue;
                }
                // Look up the actual schema from the edge's reason field.
                let actual_schema = self.load_edge_reason(project, "CALLS", callee_id)?;
                for actual in &actual_schema {
                    if actual != expected_schema {
                        result.push(ShapeViolation {
                            endpoint: ep_path.clone(),
                            expected_schema: expected_schema.clone(),
                            actual_schema: actual.clone(),
                            severity: SEVERITY_MISMATCH.to_string(),
                        });
                    }
                }
            }
        }
        // Sort by endpoint path for determinism.
        result.sort_by(|a, b| a.endpoint.cmp(&b.endpoint));
        Ok(result)
    }

    /// Analyses the impact of changing an endpoint: traces which callers
    /// would be affected by reverse-traversing `CALLS` edges from the
    /// endpoint's handler.
    ///
    /// `endpoint` is matched against both `Endpoint.path` and `Endpoint.name`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::storage::error::StorageError`] if any Cypher query
    /// fails.
    pub fn api_impact(&self, project: &str, endpoint: &str) -> StorageResult<Vec<ImpactEntry>> {
        // (a) Find the Endpoint node by path or name.
        let endpoint_id = match self.find_endpoint(project, endpoint)? {
            Some(id) => id,
            None => return Ok(Vec::new()),
        };

        // (b) Find the Handler via HANDLES edge (source = handler, target = endpoint).
        let handles = self.load_edges(project, "HANDLES")?;
        let handler_id = handles
            .iter()
            .find(|(_, target)| target == &endpoint_id)
            .map(|(source, _)| source.clone());
        let handler_id = match handler_id {
            Some(id) => id,
            None => return Ok(Vec::new()),
        };

        // (c) Find all CALLS edges pointing to the handler.
        let calls = self.load_edges(project, "CALLS")?;
        let caller_ids: Vec<String> = calls
            .iter()
            .filter(|(_, target)| target == &handler_id)
            .map(|(source, _)| source.clone())
            .collect();

        // (d) Look up caller info (name, filePath, startLine) from
        //     Function/Method/Handler tables.
        let callers = self.load_caller_info(project, &caller_ids)?;

        // (e) Build result.
        let ep_path = self
            .load_endpoint_path(project, &endpoint_id)?
            .unwrap_or_default();
        let mut result: Vec<ImpactEntry> = callers
            .into_iter()
            .map(|(name, file, line)| ImpactEntry {
                endpoint: ep_path.clone(),
                affected_caller: name,
                caller_file: file,
                caller_line: line,
            })
            .collect();
        // Sort by caller name for determinism.
        result.sort_by(|a, b| a.affected_caller.cmp(&b.affected_caller));
        Ok(result)
    }

    /// Generates a tool map: all `Tool` nodes joined with their handler
    /// functions via `HANDLES` edges.
    ///
    /// # Errors
    ///
    /// Returns [`crate::storage::error::StorageError`] if any Cypher query
    /// fails.
    pub fn tool_map(&self, project: &str) -> StorageResult<Vec<ToolEntry>> {
        // (a) Load all Tool nodes.
        let tools = self.load_tools(project)?;

        // (b) Load all Handler nodes (id → name).
        let handlers = self.load_handlers(project)?;

        // (c) Load HANDLES edges (source = handler id, target = tool id).
        let handles = self.load_edges(project, "HANDLES")?;

        // (d) Build tool → handler map.
        let mut tool_to_handler: std::collections::HashMap<String, (String, String)> =
            std::collections::HashMap::new(); // Single-line for coverage: tarpaulin attribute continuation
        for (h_id, t_id) in &handles {
            if let Some(h_name) = handlers.get(h_id) {
                tool_to_handler
                    .entry(t_id.clone())
                    .or_insert_with(|| (h_id.clone(), h_name.clone()));
            }
        }

        // (e) Build result.
        let mut result: Vec<ToolEntry> = tools
            .into_iter()
            .map(|(id, name)| {
                let (handler_id, handler_name) =
                    tool_to_handler.get(&id).cloned().unwrap_or_default();
                ToolEntry {
                    tool_name: name,
                    handler_id,
                    handler_name,
                }
            })
            .collect();
        // Sort by tool name for determinism.
        result.sort_by(|a, b| a.tool_name.cmp(&b.tool_name));
        Ok(result)
    }

    // --- Helper methods ---

    /// Loads all `Route` and `Endpoint` nodes for `project`.
    ///
    /// Returns a vector of `(id, path, method)` tuples.
    fn load_routes(&self, project: &str) -> StorageResult<Vec<(String, String, String)>> {
        let escaped = escape_cypher_string(project);
        let mut out = Vec::new();
        for label in &["Route", "Endpoint"] {
            let cypher = format!(
                "MATCH (n:{label}) WHERE n.project = '{escaped}' \
                 RETURN n.id AS id, n.path AS path, n.httpMethod AS method;"
            );
            let rows = self.storage.query(&cypher)?;
            for row in rows {
                if row.len() < 3 {
                    continue;
                }
                let id = row[0].as_str().unwrap_or_default().to_string();
                let path = row[1].as_str().unwrap_or_default().to_string();
                let method = row[2].as_str().unwrap_or_default().to_string();
                out.push((id, path, method));
            }
        }
        Ok(out)
    }

    /// Loads all `Handler` nodes for `project`.
    ///
    /// Returns a `HashMap<id, name>`.
    fn load_handlers(
        &self,
        project: &str,
    ) -> StorageResult<std::collections::HashMap<String, String>> {
        let escaped = escape_cypher_string(project);
        let cypher = format!(
            "MATCH (h:Handler) WHERE h.project = '{escaped}' \
             RETURN h.id AS id, h.name AS name;"
        );
        let rows = self.storage.query(&cypher)?;
        let mut map = std::collections::HashMap::with_capacity(rows.len());
        for row in rows {
            if row.len() < 2 {
                continue;
            }
            let id = row[0].as_str().unwrap_or_default().to_string();
            let name = row[1].as_str().unwrap_or_default().to_string();
            map.insert(id, name);
        }
        Ok(map)
    }

    /// Loads all `Middleware` nodes for `project`.
    ///
    /// Returns a `HashMap<id, name>`.
    fn load_middleware(
        &self,
        project: &str,
    ) -> StorageResult<std::collections::HashMap<String, String>> {
        let escaped = escape_cypher_string(project);
        let cypher = format!(
            "MATCH (m:Middleware) WHERE m.project = '{escaped}' \
             RETURN m.id AS id, m.name AS name;"
        );
        let rows = self.storage.query(&cypher)?;
        let mut map = std::collections::HashMap::with_capacity(rows.len());
        for row in rows {
            if row.len() < 2 {
                continue;
            }
            let id = row[0].as_str().unwrap_or_default().to_string();
            let name = row[1].as_str().unwrap_or_default().to_string();
            map.insert(id, name);
        }
        Ok(map)
    }

    /// Loads all `Tool` nodes for `project`.
    ///
    /// Returns a vector of `(id, name)` tuples.
    fn load_tools(&self, project: &str) -> StorageResult<Vec<(String, String)>> {
        let escaped = escape_cypher_string(project);
        let cypher = format!(
            "MATCH (t:Tool) WHERE t.project = '{escaped}' \
             RETURN t.id AS id, t.name AS name;"
        );
        let rows = self.storage.query(&cypher)?;
        let mut out = Vec::new();
        for row in rows {
            if row.len() < 2 {
                continue;
            }
            let id = row[0].as_str().unwrap_or_default().to_string();
            let name = row[1].as_str().unwrap_or_default().to_string();
            out.push((id, name));
        }
        Ok(out)
    }

    /// Loads `CodeRelation` edges of the given `edge_type` for `project`.
    ///
    /// Returns a vector of `(source, target)` tuples.
    fn load_edges(&self, project: &str, edge_type: &str) -> StorageResult<Vec<(String, String)>> {
        let escaped_project = escape_cypher_string(project);
        let escaped_type = escape_cypher_string(edge_type);
        let cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type = '{escaped_type}' AND e.project = '{escaped_project}' \
             RETURN e.source AS source, e.target AS target;"
        );
        let rows = self.storage.query(&cypher)?;
        let mut out = Vec::new();
        for row in rows {
            if row.len() < 2 {
                continue;
            }
            let source = row[0].as_str().unwrap_or_default().to_string();
            let target = row[1].as_str().unwrap_or_default().to_string();
            out.push((source, target));
        }
        Ok(out)
    }

    /// Loads all `Endpoint` nodes with non-empty `expectedSchema` for `project`.
    ///
    /// Returns a vector of `(id, path, expected_schema)` tuples.
    fn load_endpoints_with_schema(
        &self,
        project: &str,
    ) -> StorageResult<Vec<(String, String, String)>> {
        let escaped = escape_cypher_string(project);
        let cypher = format!(
            "MATCH (e:Endpoint) WHERE e.project = '{escaped}' \
             RETURN e.id AS id, e.path AS path, e.expectedSchema AS expected_schema;"
        );
        let rows = self.storage.query(&cypher)?;
        let mut out = Vec::new();
        for row in rows {
            if row.len() < 3 {
                continue;
            }
            let id = row[0].as_str().unwrap_or_default().to_string();
            let path = row[1].as_str().unwrap_or_default().to_string();
            let expected = row[2].as_str().unwrap_or_default().to_string();
            out.push((id, path, expected));
        }
        Ok(out)
    }

    /// Loads the `reason` field (actual schema) from `CALLS` edges pointing
    /// to `target_id`.
    fn load_edge_reason(
        &self,
        project: &str,
        edge_type: &str,
        target_id: &str,
    ) -> StorageResult<Vec<String>> {
        let escaped_project = escape_cypher_string(project);
        let escaped_type = escape_cypher_string(edge_type);
        let escaped_target = escape_cypher_string(target_id);
        let cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type = '{escaped_type}' AND e.project = '{escaped_project}' \
             AND e.target = '{escaped_target}' RETURN e.reason AS reason;"
        );
        let rows = self.storage.query(&cypher)?;
        let mut out = Vec::new();
        for row in rows {
            if let Some(reason) = row.first().and_then(|v| v.as_str()) {
                if !reason.is_empty() {
                    out.push(reason.to_string());
                }
            }
        }
        Ok(out)
    }

    /// Finds an `Endpoint` node by `path` or `name` for `project`.
    ///
    /// Returns the endpoint id if found.
    fn find_endpoint(&self, project: &str, endpoint: &str) -> StorageResult<Option<String>> {
        let escaped_project = escape_cypher_string(project);
        let escaped_endpoint = escape_cypher_string(endpoint);
        // Try by path first, then by name.
        for field in &["path", "name"] {
            let cypher = format!(
                "MATCH (e:Endpoint) WHERE e.project = '{escaped_project}' \
                 AND e.{field} = '{escaped_endpoint}' RETURN e.id AS id;"
            );
            let rows = self.storage.query(&cypher)?;
            if let Some(row) = rows.into_iter().next() {
                if let Some(id) = row.first().and_then(|v| v.as_str()) {
                    return Ok(Some(id.to_string()));
                }
            }
        }
        Ok(None)
    }

    /// Loads the `path` of an `Endpoint` by `id`.
    fn load_endpoint_path(
        &self,
        project: &str,
        endpoint_id: &str,
    ) -> StorageResult<Option<String>> {
        let escaped_project = escape_cypher_string(project);
        let escaped_id = escape_cypher_string(endpoint_id);
        let cypher = format!(
            "MATCH (e:Endpoint) WHERE e.project = '{escaped_project}' \
             AND e.id = '{escaped_id}' RETURN e.path AS path;"
        );
        let rows = self.storage.query(&cypher)?;
        if let Some(row) = rows.into_iter().next() {
            if let Some(path) = row.first().and_then(|v| v.as_str()) {
                return Ok(Some(path.to_string()));
            }
        }
        Ok(None)
    }

    /// Loads caller info (name, filePath, startLine) for the given `caller_ids`.
    ///
    /// Searches `Function`, `Method`, and `Handler` tables.
    fn load_caller_info(
        &self,
        project: &str,
        caller_ids: &[String],
    ) -> StorageResult<Vec<(String, String, u32)>> {
        if caller_ids.is_empty() {
            return Ok(Vec::new());
        }
        let escaped_project = escape_cypher_string(project);
        let mut out = Vec::new();
        for table in &["Function", "Method", "Handler"] {
            let cypher = format!(
                "MATCH (n:{table}) WHERE n.project = '{escaped_project}' \
                 RETURN n.id AS id, n.name AS name, n.filePath AS file_path, n.startLine AS start_line;"
            );
            let rows = self.storage.query(&cypher)?;
            for row in rows {
                if row.len() < 4 {
                    continue;
                }
                let id = row[0].as_str().unwrap_or_default().to_string();
                if !caller_ids.contains(&id) {
                    continue;
                }
                let name = row[1].as_str().unwrap_or_default().to_string();
                let file = row[2].as_str().unwrap_or_default().to_string();
                let line = row[3]
                    .as_i64()
                    .map(|v| v as u32)
                    .or_else(|| row[3].as_u64().map(|v| v as u32))
                    .unwrap_or(0);
                out.push((name, file, line));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, Kit, KitBootstrapConfig, StorageKey};
    use tempfile::TempDir;

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("api_review_testdb");
        std::mem::forget(dir);
        path
    }

    /// Builds a Kit backed by an on-disk database at `db`.
    fn build_kit_for_db(db: &std::path::Path) -> Kit {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        build_kit(&config).expect("build_kit")
    }

    /// Returns the `dyn Storage` capability from `kit`.
    fn storage(kit: &Kit) -> std::sync::Arc<dyn crate::storage::capability::Storage> {
        kit.require::<StorageKey>().expect("require_storage")
    }

    /// Creates a Route node.
    fn create_route(kit: &Kit, id: &str, project: &str, path: &str, method: &str) {
        let storage = storage(kit);
        let cypher = format!(
            "CREATE (:Route {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '', startLine: 0, endLine: 0, httpMethod: '{}', path: '{}', parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(path),
            escape_cypher_string(path),
            escape_cypher_string(method),
            escape_cypher_string(path),
        );
        storage.execute(&cypher).expect("create route");
    }

    /// Creates an Endpoint node with an expected schema.
    fn create_endpoint(
        kit: &Kit,
        id: &str,
        project: &str,
        path: &str,
        method: &str,
        expected_schema: &str,
    ) {
        let storage = storage(kit);
        let cypher = format!(
            "CREATE (:Endpoint {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '', startLine: 0, endLine: 0, httpMethod: '{}', path: '{}', \
             expectedSchema: '{}', parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(path),
            escape_cypher_string(path),
            escape_cypher_string(method),
            escape_cypher_string(path),
            escape_cypher_string(expected_schema),
        );
        storage.execute(&cypher).expect("create endpoint");
    }

    /// Creates a Handler node.
    fn create_handler(kit: &Kit, id: &str, project: &str, name: &str) {
        let storage = storage(kit);
        let cypher = format!(
            "CREATE (:Handler {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '', startLine: 0, endLine: 0, signature: '', returnType: '', \
             isExported: false, docstring: '', content: '', parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(name),
            escape_cypher_string(name),
        );
        storage.execute(&cypher).expect("create handler");
    }

    /// Creates a Middleware node.
    fn create_middleware(kit: &Kit, id: &str, project: &str, name: &str) {
        let storage = storage(kit);
        let cypher = format!(
            "CREATE (:Middleware {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '', startLine: 0, endLine: 0, signature: '', returnType: '', \
             isExported: false, docstring: '', content: '', parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(name),
            escape_cypher_string(name),
        );
        storage.execute(&cypher).expect("create middleware");
    }

    /// Creates a Tool node.
    fn create_tool(kit: &Kit, id: &str, project: &str, name: &str) {
        let storage = storage(kit);
        let cypher = format!(
            "CREATE (:Tool {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '', toolType: 'mcp', parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(name),
            escape_cypher_string(name),
        );
        storage.execute(&cypher).expect("create tool");
    }

    /// Creates a Function node.
    fn create_function(kit: &Kit, id: &str, project: &str, name: &str, file: &str, line: u32) {
        let storage = storage(kit);
        let end_line = line + 10;
        let cypher = format!(
            "CREATE (:Function {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '{}', startLine: {}, endLine: {}, signature: '', returnType: '', \
             isExported: false, docstring: '', content: '', parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(name),
            escape_cypher_string(name),
            escape_cypher_string(file),
            line,
            end_line,
        );
        storage.execute(&cypher).expect("create function");
    }

    /// Creates a CodeRelation edge.
    fn create_edge(
        kit: &Kit,
        id: &str,
        source: &str,
        target: &str,
        edge_type: &str,
        project: &str,
    ) {
        let storage = storage(kit);
        let cypher = format!(
            "CREATE (:CodeRelation {{id: '{}', source: '{}', target: '{}', type: '{}', \
             confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: '{}'}});",
            escape_cypher_string(id),
            escape_cypher_string(source),
            escape_cypher_string(target),
            escape_cypher_string(edge_type),
            escape_cypher_string(project),
        );
        storage.execute(&cypher).expect("create edge");
    }

    /// Creates a CodeRelation edge with a reason (used for actual schema).
    fn create_edge_with_reason(
        kit: &Kit,
        id: &str,
        source: &str,
        target: &str,
        edge_type: &str,
        project: &str,
        reason: &str,
    ) {
        let storage = storage(kit);
        let cypher = format!(
            "CREATE (:CodeRelation {{id: '{}', source: '{}', target: '{}', type: '{}', \
             confidence: 1.0, confidenceTier: 'High', reason: '{}', startLine: 1, project: '{}'}});",
            escape_cypher_string(id),
            escape_cypher_string(source),
            escape_cypher_string(target),
            escape_cypher_string(edge_type),
            escape_cypher_string(reason),
            escape_cypher_string(project),
        );
        storage.execute(&cypher).expect("create edge with reason");
    }

    // --- route_map tests ---

    #[test]
    fn route_map_returns_empty_for_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer.route_map("demo").expect("route_map");
        assert!(result.is_empty(), "empty DB should yield no routes");
    }

    #[test]
    fn route_map_lists_endpoints_with_handlers() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_route(&kit, "r1", "demo", "/api/users", "GET");
        create_handler(&kit, "h1", "demo", "list_users");
        create_edge(&kit, "e1", "h1", "r1", "HANDLES", "demo");

        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer.route_map("demo").expect("route_map");
        assert_eq!(result.len(), 1, "should have 1 route");
        let entry = &result[0];
        assert_eq!(entry.path, "/api/users");
        assert_eq!(entry.method, "GET");
        assert_eq!(entry.handler_id, "h1");
        assert_eq!(entry.handler_name, "list_users");
        assert!(entry.middleware.is_empty(), "no middleware");
    }

    #[test]
    fn route_map_includes_middleware() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_route(&kit, "r1", "demo", "/api/users", "GET");
        create_handler(&kit, "h1", "demo", "list_users");
        create_middleware(&kit, "m1", "demo", "auth_middleware");
        create_edge(&kit, "e1", "h1", "r1", "HANDLES", "demo");
        create_edge(&kit, "e2", "m1", "h1", "USES", "demo");

        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer.route_map("demo").expect("route_map");
        assert_eq!(result.len(), 1, "should have 1 route");
        let entry = &result[0];
        assert_eq!(entry.handler_name, "list_users");
        assert_eq!(entry.middleware.len(), 1, "should have 1 middleware");
        assert_eq!(entry.middleware[0], "auth_middleware");
    }

    #[test]
    fn route_map_route_without_handler() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_route(&kit, "r1", "demo", "/api/orphan", "POST");

        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer.route_map("demo").expect("route_map");
        assert_eq!(result.len(), 1, "should have 1 route");
        let entry = &result[0];
        assert_eq!(entry.path, "/api/orphan");
        assert!(entry.handler_id.is_empty(), "handler_id should be empty");
        assert!(
            entry.handler_name.is_empty(),
            "handler_name should be empty"
        );
    }

    #[test]
    fn route_map_multiple_routes() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_route(&kit, "r1", "demo", "/api/users", "GET");
        create_route(&kit, "r2", "demo", "/api/products", "POST");
        create_handler(&kit, "h1", "demo", "list_users");
        create_handler(&kit, "h2", "demo", "create_product");
        create_edge(&kit, "e1", "h1", "r1", "HANDLES", "demo");
        create_edge(&kit, "e2", "h2", "r2", "HANDLES", "demo");

        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer.route_map("demo").expect("route_map");
        assert_eq!(result.len(), 2, "should have 2 routes");
        // Sorted by path.
        assert_eq!(result[0].path, "/api/products");
        assert_eq!(result[1].path, "/api/users");
    }

    #[test]
    fn route_map_filters_by_project() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_route(&kit, "r1", "demo", "/api/users", "GET");
        create_route(&kit, "r2", "other", "/api/products", "POST");

        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer.route_map("demo").expect("route_map");
        assert_eq!(result.len(), 1, "should only see demo's routes");
        assert_eq!(result[0].path, "/api/users");
    }

    // --- shape_check tests ---

    #[test]
    fn shape_check_returns_empty_for_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer.shape_check("demo").expect("shape_check");
        assert!(result.is_empty(), "empty DB should yield no violations");
    }

    #[test]
    fn shape_check_finds_mismatch() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_endpoint(
            &kit,
            "e1",
            "demo",
            "/api/users",
            "GET",
            r#"{"name":"string"}"#,
        );
        create_function(&kit, "f1", "demo", "caller", "/src/app.rs", 10);
        // CALLS edge from caller to endpoint with actual schema in reason.
        create_edge_with_reason(
            &kit,
            "cr1",
            "f1",
            "e1",
            "CALLS",
            "demo",
            r#"{"name":"number"}"#,
        );

        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer.shape_check("demo").expect("shape_check");
        assert_eq!(result.len(), 1, "should have 1 violation");
        let v = &result[0];
        assert_eq!(v.endpoint, "/api/users");
        assert_eq!(v.expected_schema, r#"{"name":"string"}"#);
        assert_eq!(v.actual_schema, r#"{"name":"number"}"#);
        assert_eq!(v.severity, "mismatch");
    }

    #[test]
    fn shape_check_no_violation_when_schemas_match() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_endpoint(
            &kit,
            "e1",
            "demo",
            "/api/users",
            "GET",
            r#"{"name":"string"}"#,
        );
        create_function(&kit, "f1", "demo", "caller", "/src/app.rs", 10);
        create_edge_with_reason(
            &kit,
            "cr1",
            "f1",
            "e1",
            "CALLS",
            "demo",
            r#"{"name":"string"}"#,
        );

        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer.shape_check("demo").expect("shape_check");
        assert!(
            result.is_empty(),
            "matching schemas should yield no violations"
        );
    }

    #[test]
    fn shape_check_skips_endpoint_without_expected_schema() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Endpoint with empty expectedSchema.
        create_endpoint(&kit, "e1", "demo", "/api/users", "GET", "");
        create_function(&kit, "f1", "demo", "caller", "/src/app.rs", 10);
        create_edge_with_reason(
            &kit,
            "cr1",
            "f1",
            "e1",
            "CALLS",
            "demo",
            r#"{"name":"number"}"#,
        );

        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer.shape_check("demo").expect("shape_check");
        assert!(result.is_empty(), "no expectedSchema → no violation");
    }

    #[test]
    fn shape_check_filters_by_project() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_endpoint(
            &kit,
            "e1",
            "demo",
            "/api/users",
            "GET",
            r#"{"name":"string"}"#,
        );
        create_endpoint(
            &kit,
            "e2",
            "other",
            "/api/products",
            "GET",
            r#"{"name":"string"}"#,
        );
        create_function(&kit, "f1", "demo", "caller", "/src/app.rs", 10);
        create_edge_with_reason(
            &kit,
            "cr1",
            "f1",
            "e1",
            "CALLS",
            "demo",
            r#"{"name":"number"}"#,
        );

        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer.shape_check("demo").expect("shape_check");
        assert_eq!(result.len(), 1, "should only see demo's violations");
        assert_eq!(result[0].endpoint, "/api/users");
    }

    // --- api_impact tests ---

    #[test]
    fn api_impact_returns_empty_for_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer
            .api_impact("demo", "/api/users")
            .expect("api_impact");
        assert!(result.is_empty(), "empty DB should yield no impact");
    }

    #[test]
    fn api_impact_traces_callers() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_endpoint(&kit, "e1", "demo", "/api/users", "GET", "");
        create_handler(&kit, "h1", "demo", "list_users");
        // HANDLES edge: handler → endpoint.
        create_edge(&kit, "he1", "h1", "e1", "HANDLES", "demo");
        // 2 callers calling the handler.
        create_function(&kit, "f1", "demo", "caller_a", "/src/a.rs", 10);
        create_function(&kit, "f2", "demo", "caller_b", "/src/b.rs", 20);
        create_edge(&kit, "ce1", "f1", "h1", "CALLS", "demo");
        create_edge(&kit, "ce2", "f2", "h1", "CALLS", "demo");

        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer
            .api_impact("demo", "/api/users")
            .expect("api_impact");
        assert_eq!(result.len(), 2, "should have 2 impact entries");
        // Sorted by caller name.
        assert_eq!(result[0].affected_caller, "caller_a");
        assert_eq!(result[0].caller_file, "/src/a.rs");
        assert_eq!(result[0].caller_line, 10);
        assert_eq!(result[1].affected_caller, "caller_b");
        assert_eq!(result[1].caller_file, "/src/b.rs");
        assert_eq!(result[1].caller_line, 20);
        // All entries should have the endpoint path.
        for entry in &result {
            assert_eq!(entry.endpoint, "/api/users");
        }
    }

    #[test]
    fn api_impact_no_callers_returns_empty() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_endpoint(&kit, "e1", "demo", "/api/users", "GET", "");
        create_handler(&kit, "h1", "demo", "list_users");
        create_edge(&kit, "he1", "h1", "e1", "HANDLES", "demo");
        // No CALLS edges to handler.

        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer
            .api_impact("demo", "/api/users")
            .expect("api_impact");
        assert!(result.is_empty(), "no callers → no impact");
    }

    #[test]
    fn api_impact_endpoint_not_found_returns_empty() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_endpoint(&kit, "e1", "demo", "/api/users", "GET", "");

        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer
            .api_impact("demo", "/api/nonexistent")
            .expect("api_impact");
        assert!(result.is_empty(), "nonexistent endpoint → no impact");
    }

    #[test]
    fn api_impact_endpoint_without_handler_returns_empty() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_endpoint(&kit, "e1", "demo", "/api/users", "GET", "");
        // No HANDLES edge.

        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer
            .api_impact("demo", "/api/users")
            .expect("api_impact");
        assert!(result.is_empty(), "no handler → no impact");
    }

    #[test]
    fn api_impact_matches_by_name() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Endpoint with name "users_api" and path "/api/users".
        let storage = storage(&kit);
        let cypher = format!(
            "CREATE (:Endpoint {{id: 'e1', project: 'demo', name: 'users_api', qualifiedName: 'users_api', \
             filePath: '', startLine: 0, endLine: 0, httpMethod: 'GET', path: '/api/users', \
             expectedSchema: '', parentQn: ''}});"
        );
        storage.execute(&cypher).expect("create endpoint");
        create_handler(&kit, "h1", "demo", "list_users");
        create_edge(&kit, "he1", "h1", "e1", "HANDLES", "demo");
        create_function(&kit, "f1", "demo", "caller", "/src/a.rs", 5);
        create_edge(&kit, "ce1", "f1", "h1", "CALLS", "demo");

        let reviewer = ApiReviewer::new(&*storage);
        // Match by name (not path).
        let result = reviewer
            .api_impact("demo", "users_api")
            .expect("api_impact");
        assert_eq!(result.len(), 1, "should find 1 caller by name match");
    }

    // --- tool_map tests ---

    #[test]
    fn tool_map_returns_empty_for_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer.tool_map("demo").expect("tool_map");
        assert!(result.is_empty(), "empty DB should yield no tools");
    }

    #[test]
    fn tool_map_lists_tools() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_tool(&kit, "t1", "demo", "query");
        create_handler(&kit, "h1", "demo", "query_handler");
        create_edge(&kit, "e1", "h1", "t1", "HANDLES", "demo");

        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer.tool_map("demo").expect("tool_map");
        assert_eq!(result.len(), 1, "should have 1 tool");
        let entry = &result[0];
        assert_eq!(entry.tool_name, "query");
        assert_eq!(entry.handler_id, "h1");
        assert_eq!(entry.handler_name, "query_handler");
    }

    #[test]
    fn tool_map_tool_without_handler() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_tool(&kit, "t1", "demo", "orphan_tool");

        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer.tool_map("demo").expect("tool_map");
        assert_eq!(result.len(), 1, "should have 1 tool");
        let entry = &result[0];
        assert_eq!(entry.tool_name, "orphan_tool");
        assert!(entry.handler_id.is_empty(), "handler_id should be empty");
        assert!(
            entry.handler_name.is_empty(),
            "handler_name should be empty"
        );
    }

    #[test]
    fn tool_map_multiple_tools() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_tool(&kit, "t1", "demo", "query");
        create_tool(&kit, "t2", "demo", "search");
        create_handler(&kit, "h1", "demo", "query_handler");
        create_handler(&kit, "h2", "demo", "search_handler");
        create_edge(&kit, "e1", "h1", "t1", "HANDLES", "demo");
        create_edge(&kit, "e2", "h2", "t2", "HANDLES", "demo");

        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer.tool_map("demo").expect("tool_map");
        assert_eq!(result.len(), 2, "should have 2 tools");
        // Sorted by tool name.
        assert_eq!(result[0].tool_name, "query");
        assert_eq!(result[1].tool_name, "search");
    }

    #[test]
    fn tool_map_filters_by_project() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_tool(&kit, "t1", "demo", "query");
        create_tool(&kit, "t2", "other", "search");

        let storage = storage(&kit);
        let reviewer = ApiReviewer::new(&*storage);
        let result = reviewer.tool_map("demo").expect("tool_map");
        assert_eq!(result.len(), 1, "should only see demo's tools");
        assert_eq!(result[0].tool_name, "query");
    }
}
