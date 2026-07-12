// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Architecture overview.
//!
//! Produces a high-level summary of a project's structure by aggregating over
//! the existing LadybugDB graph:
//! - **Languages**: file count + symbol count per source language
//! - **Packages**: symbol count grouped by package prefix (first 2 components
//!   of the qualified name, e.g. `com.example`)
//! - **Entry points**: functions matching default entry patterns
//!   (`main`, `Main`, `__main__`)
//! - **Routes**: HTTP routes joined with their handler functions via
//!   `HANDLES` edges in the `CodeRelation` table
//! - **Hotspots**: functions ranked by incoming `CALLS` edge count (top 10)
//!
//! # Algorithm
//!
//! Each section issues one or more Cypher queries against the `&dyn Storage`
//! capability and joins results in Rust. LadybugDB's Cypher subset does not
//! support `GROUP BY`, multi-label `WHERE (n:A OR n:B)` expressions, or
//! `UNION` — so we issue separate queries per node label and aggregate in
//! Rust.

use crate::analysis::cross_service::{CrossServiceDetector, ServiceProtocol};
use crate::storage::capability::Storage;
use crate::storage::error::Result as StorageResult;
use crate::storage::schema::escape_cypher_string;
use serde::Serialize;

/// Default glob patterns for entry-point function names.
const DEFAULT_ENTRY_PATTERNS: &[&str] = &["main", "Main", "__main__"];

/// Maximum number of hotspot entries to return (spec: "最多返回 10 条").
const HOTSPOT_LIMIT: usize = 10;

/// Symbol tables queried for package grouping. Each has a `qualifiedName`
/// column. LadybugDB does not support `UNION`, so we issue one query per
/// table and merge in Rust.
const PACKAGE_TABLES: &[&str] = &[
    "Function",
    "Method",
    "Class",
    "Struct",
    "Enum",
    "Trait",
    "Interface",
    "Namespace",
];

/// A complete architecture overview for a project.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ArchitectureOverview {
    /// Language distribution (file count + symbol count per language).
    pub languages: Vec<LanguageStat>,
    /// Package/module distribution (symbol count per package prefix).
    pub packages: Vec<PackageStat>,
    /// Entry-point functions (matching default entry patterns).
    pub entry_points: Vec<EntryPoint>,
    /// HTTP routes joined with their handler functions.
    pub routes: Vec<RouteStat>,
    /// High-indegree functions (top 10 by caller count).
    pub hotspots: Vec<HotspotStat>,
    /// Module boundaries detected by grouping files by directory.
    pub module_boundaries: Vec<ModuleBoundary>,
    /// Directed module dependencies with circular-dep flags.
    pub dependency_directions: Vec<DepDirection>,
    /// Architectural layers (Controller/Service/Repository/Model).
    pub layers: Vec<LayerInfo>,
    /// Cross-service dependencies (reserved for future use).
    pub cross_service_deps: Vec<CrossServiceDep>,
}

/// Language statistics.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LanguageStat {
    /// Source language (e.g. `rust`, `python`).
    pub language: String,
    /// Number of `File` nodes with this language.
    pub file_count: u32,
    /// Number of symbol nodes (`Function`/`Method`) in files of this language.
    pub symbol_count: u32,
}

/// Package statistics.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PackageStat {
    /// Package prefix (first 2 components of qualified name, e.g. `com.example`).
    pub package: String,
    /// Number of symbols in this package.
    pub symbol_count: u32,
}

/// An entry-point function.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EntryPoint {
    /// Short function name (e.g. `main`).
    pub name: String,
    /// Fully-qualified name (e.g. `demo.main`).
    pub qualified_name: String,
    /// Source file path.
    pub file_path: String,
    /// 1-based start line.
    pub line: u32,
}

/// An HTTP route joined with its handler.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RouteStat {
    /// Route path (e.g. `/api/users`).
    pub path: String,
    /// HTTP method (e.g. `GET`, `POST`). Empty if not set.
    pub method: String,
    /// Handler function name. Empty if no handler linked.
    pub handler: String,
}

/// A hotspot function (high incoming CALLS edge count).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HotspotStat {
    /// Short function name.
    pub name: String,
    /// Fully-qualified name.
    pub qualified_name: String,
    /// Number of incoming CALLS edges (caller count).
    pub caller_count: u32,
}

/// A module boundary detected by grouping files by directory.
///
/// `cohesion` = internal CALLS edges / (internal + external CALLS edges).
/// A module with no external dependencies has `cohesion = 1.0`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ModuleBoundary {
    /// Module name (directory path).
    pub module_name: String,
    /// File paths belonging to this module.
    pub members: Vec<String>,
    /// CALLS edges from external modules into this module.
    pub incoming_deps: u32,
    /// CALLS edges from this module to external modules.
    pub outgoing_deps: u32,
    /// Internal edges / total edges (0.0–1.0).
    pub cohesion: f64,
}

/// A directed dependency between two modules.
///
/// `is_circular` is `true` when the dependency participates in a cycle
/// detected via DFS coloring.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DepDirection {
    /// Source module name.
    pub from_module: String,
    /// Target module name.
    pub to_module: String,
    /// Whether this edge is part of a cycle.
    pub is_circular: bool,
}

/// A logical architectural layer (Controller/Service/Repository/Model).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LayerInfo {
    /// Layer name: `"Controller"`, `"Service"`, `"Repository"`, `"Model"`.
    pub layer: String,
    /// Member qualified names in this layer.
    pub members: Vec<String>,
}

/// A cross-service dependency detected between two code symbols.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CrossServiceDep {
    /// Source module name.
    pub from_module: String,
    /// Target module name.
    pub to_module: String,
    /// Communication protocol (e.g. `"HTTP"`, `"gRPC"`).
    pub protocol: String,
}

/// Produces an [`ArchitectureOverview`] for a project.
pub struct ArchitectureAnalyzer<'a> {
    storage: &'a dyn Storage,
}

impl<'a> ArchitectureAnalyzer<'a> {
    /// Creates a new analyzer backed by the given storage capability.
    #[must_use]
    pub fn new(storage: &'a dyn Storage) -> Self {
        Self { storage }
    }

    /// Returns the architecture overview for `project`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::storage::error::StorageError`] if any Cypher query
    /// fails.
    pub fn overview(&self, project: &str) -> StorageResult<ArchitectureOverview> {
        let languages = self.load_languages(project)?;
        let packages = self.load_packages(project)?;
        let entry_points = self.load_entry_points(project)?;
        let routes = self.load_routes(project)?;
        let hotspots = self.load_hotspots(project)?;
        let module_boundaries = self.detect_module_boundaries(project)?;
        let dependency_directions = self.analyze_dependency_directions(project)?;
        let layers = self.detect_layers(project)?;
        let cross_service_deps = self.load_cross_service_deps(project)?;
        Ok(ArchitectureOverview {
            languages,
            packages,
            entry_points,
            routes,
            hotspots,
            module_boundaries,
            dependency_directions,
            layers,
            cross_service_deps,
        })
    }

    /// Loads language statistics: file count + symbol count per language.
    ///
    /// Symbol count is computed by joining `Function`/`Method` nodes with the
    /// `File` table via `filePath` (polyglot projects are handled correctly).
    fn load_languages(&self, project: &str) -> StorageResult<Vec<LanguageStat>> {
        let escaped = escape_cypher_string(project);
        // (a) Load all File nodes with their language + filePath.
        let file_cypher = format!(
            "MATCH (f:File) WHERE f.project = '{escaped}' \
             RETURN f.language AS language, f.filePath AS file_path;"
        );
        let file_rows = self.storage.query(&file_cypher)?;

        // Build: language -> file_count, and filePath -> language map.
        // Single-line for coverage: tarpaulin attribute continuation
        let mut lang_file_count: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        let mut path_to_lang: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for row in file_rows {
            if row.len() < 2 {
                continue;
            }
            let language = row[0].as_str().unwrap_or_default().to_string();
            let file_path = row[1].as_str().unwrap_or_default().to_string();
            *lang_file_count.entry(language.clone()).or_insert(0) += 1;
            path_to_lang.insert(file_path, language);
        }

        // (b) Count symbols (Function + Method) per language via filePath join.
        let mut lang_symbol_count: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new(); // Single-line for coverage: tarpaulin attribute continuation
        for table in &["Function", "Method"] {
            let cypher = format!(
                "MATCH (n:{table}) WHERE n.project = '{escaped}' \
                 RETURN n.filePath AS file_path;"
            );
            let rows = self.storage.query(&cypher)?;
            for row in rows {
                if let Some(path) = row.first().and_then(|v| v.as_str()) {
                    if let Some(lang) = path_to_lang.get(path) {
                        *lang_symbol_count.entry(lang.clone()).or_insert(0) += 1;
                    }
                }
            }
        }

        // (c) Build the result vector, sorted by language for determinism.
        let mut result: Vec<LanguageStat> = lang_file_count
            .into_iter()
            .map(|(language, file_count)| LanguageStat {
                file_count,
                symbol_count: *lang_symbol_count.get(&language).unwrap_or(&0),
                language,
            })
            .collect();
        result.sort_by(|a, b| a.language.cmp(&b.language));
        Ok(result)
    }

    /// Loads package statistics: symbol count grouped by package prefix.
    ///
    /// Package prefix = first 2 components of `qualifiedName` split by `.`
    /// (e.g. `com.example.Foo` → `com.example`).
    fn load_packages(&self, project: &str) -> StorageResult<Vec<PackageStat>> {
        let escaped = escape_cypher_string(project);
        let mut pkg_counts: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new(); // Single-line for coverage: tarpaulin attribute continuation
        for table in PACKAGE_TABLES {
            let cypher = format!(
                "MATCH (n:{table}) WHERE n.project = '{escaped}' \
                 RETURN n.qualifiedName AS qualified_name;"
            );
            let rows = self.storage.query(&cypher)?;
            for row in rows {
                if let Some(qn) = row.first().and_then(|v| v.as_str()) {
                    if let Some(pkg) = package_prefix(qn) {
                        *pkg_counts.entry(pkg.to_string()).or_insert(0) += 1;
                    }
                }
            }
        }
        let mut result: Vec<PackageStat> = pkg_counts
            .into_iter()
            .map(|(package, symbol_count)| PackageStat {
                package,
                symbol_count,
            })
            .collect();
        // Sort by symbol_count desc, then by package name for determinism.
        result.sort_by(|a, b| {
            b.symbol_count
                .cmp(&a.symbol_count)
                .then_with(|| a.package.cmp(&b.package))
        });
        Ok(result)
    }

    /// Loads entry-point functions (matching default entry patterns).
    fn load_entry_points(&self, project: &str) -> StorageResult<Vec<EntryPoint>> {
        let escaped = escape_cypher_string(project);
        let cypher = format!(
            "MATCH (f:Function) WHERE f.project = '{escaped}' \
             RETURN f.name AS name, f.qualifiedName AS qualified_name, \
             f.filePath AS file_path, f.startLine AS start_line;"
        );
        let rows = self.storage.query(&cypher)?;
        let mut result = Vec::new();
        for row in rows {
            if row.len() < 4 {
                continue;
            }
            let name = row[0].as_str().unwrap_or_default().to_string();
            if !DEFAULT_ENTRY_PATTERNS.contains(&name.as_str()) {
                continue;
            }
            let qualified_name = row[1].as_str().unwrap_or_default().to_string();
            let file_path = row[2].as_str().unwrap_or_default().to_string();
            let line = row[3]
                .as_i64()
                .map(|v| v as u32)
                .or_else(|| row[3].as_u64().map(|v| v as u32))
                .unwrap_or(0);
            result.push(EntryPoint {
                name,
                qualified_name,
                file_path,
                line,
            });
        }
        // Sort by qualified name for determinism.
        result.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
        Ok(result)
    }

    /// Loads HTTP routes joined with their handler functions.
    ///
    /// Routes and handlers are linked via `CodeRelation` rows with
    /// `type = 'HANDLES'` (source = handler id, target = route id).
    fn load_routes(&self, project: &str) -> StorageResult<Vec<RouteStat>> {
        let escaped = escape_cypher_string(project);
        // (a) Load all Route nodes.
        let route_cypher = format!(
            "MATCH (r:Route) WHERE r.project = '{escaped}' \
             RETURN r.id AS id, r.path AS path, r.httpMethod AS method;"
        );
        let route_rows = self.storage.query(&route_cypher)?;
        let mut routes: Vec<(String, String, String)> = Vec::new(); // (id, path, method)
        for row in route_rows {
            if row.len() < 3 {
                continue;
            }
            let id = row[0].as_str().unwrap_or_default().to_string();
            let path = row[1].as_str().unwrap_or_default().to_string();
            let method = row[2].as_str().unwrap_or_default().to_string();
            routes.push((id, path, method));
        }

        // (b) Load all Handler nodes (id → name).
        let handler_cypher = format!(
            "MATCH (h:Handler) WHERE h.project = '{escaped}' \
             RETURN h.id AS id, h.name AS name;"
        );
        let handler_rows = self.storage.query(&handler_cypher)?;
        let mut handlers: std::collections::HashMap<String, String> =
            std::collections::HashMap::new(); // Single-line for coverage: tarpaulin attribute continuation
        for row in handler_rows {
            if row.len() < 2 {
                continue;
            }
            let id = row[0].as_str().unwrap_or_default().to_string();
            let name = row[1].as_str().unwrap_or_default().to_string();
            handlers.insert(id, name);
        }

        // (c) Load HANDLES edges (source = handler id, target = route id).
        let edge_cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type = 'HANDLES' AND e.project = '{escaped}' \
             RETURN e.source AS source, e.target AS target;"
        );
        let edge_rows = self.storage.query(&edge_cypher)?;
        let mut route_to_handler: std::collections::HashMap<String, String> =
            std::collections::HashMap::new(); // Single-line for coverage: tarpaulin attribute continuation
        for row in edge_rows {
            if row.len() < 2 {
                continue;
            }
            let source = row[0].as_str().unwrap_or_default().to_string();
            let target = row[1].as_str().unwrap_or_default().to_string();
            if let Some(handler_name) = handlers.get(&source) {
                route_to_handler.insert(target, handler_name.clone());
            }
        }

        // (d) Build result: for each route, look up its handler (if any).
        let mut result: Vec<RouteStat> = routes
            .into_iter()
            .map(|(id, path, method)| RouteStat {
                path,
                method,
                handler: route_to_handler.get(&id).cloned().unwrap_or_default(),
            })
            .collect();
        // Sort by path for determinism.
        result.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(result)
    }

    /// Loads hotspot functions: top 10 by incoming CALLS edge count.
    ///
    /// LadybugDB does not support `GROUP BY`, so we load all CALLS edges and
    /// count in Rust.
    fn load_hotspots(&self, project: &str) -> StorageResult<Vec<HotspotStat>> {
        let escaped = escape_cypher_string(project);
        // (a) Count CALLS edges per target id.
        let calls_cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type = 'CALLS' AND e.project = '{escaped}' \
             RETURN e.target AS target;"
        );
        let calls_rows = self.storage.query(&calls_cypher)?;
        let mut caller_counts: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new(); // Single-line for coverage: tarpaulin attribute continuation
        for row in calls_rows {
            if let Some(target) = row.first().and_then(|v| v.as_str()) {
                *caller_counts.entry(target.to_string()).or_insert(0) += 1;
            }
        }

        // (b) Load Function + Method nodes for name/qualifiedName.
        let mut id_to_info: std::collections::HashMap<String, (String, String)> =
            std::collections::HashMap::new(); // Single-line for coverage: tarpaulin attribute continuation
        for table in &["Function", "Method"] {
            let cypher = format!(
                "MATCH (n:{table}) WHERE n.project = '{escaped}' \
                 RETURN n.id AS id, n.name AS name, n.qualifiedName AS qualified_name;"
            );
            let rows = self.storage.query(&cypher)?;
            for row in rows {
                if row.len() < 3 {
                    continue;
                }
                let id = row[0].as_str().unwrap_or_default().to_string();
                let name = row[1].as_str().unwrap_or_default().to_string();
                let qn = row[2].as_str().unwrap_or_default().to_string();
                id_to_info.insert(id, (name, qn));
            }
        }

        // (c) Build hotspot stats: for each target with callers, look up name.
        let mut result: Vec<HotspotStat> = caller_counts
            .into_iter()
            .filter_map(|(id, count)| {
                id_to_info.get(&id).map(|(name, qn)| HotspotStat {
                    name: name.clone(),
                    qualified_name: qn.clone(),
                    caller_count: count,
                })
            })
            .collect();
        // Sort by caller_count desc, then by qualified_name for determinism.
        result.sort_by(|a, b| {
            b.caller_count
                .cmp(&a.caller_count)
                .then_with(|| a.qualified_name.cmp(&b.qualified_name))
        });
        // Truncate to top 10.
        result.truncate(HOTSPOT_LIMIT);
        Ok(result)
    }

    /// Detects module boundaries by grouping files by directory and counting
    /// internal vs external CALLS edges.
    ///
    /// # Errors
    ///
    /// Returns [`crate::storage::error::StorageError`] if any Cypher query
    /// fails.
    fn detect_module_boundaries(&self, project: &str) -> StorageResult<Vec<ModuleBoundary>> {
        let escaped = escape_cypher_string(project);

        // (a) Load Function + Method nodes → id → filePath, and group files by module.
        let mut id_to_path: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut module_files: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for table in &["Function", "Method"] {
            let cypher = format!(
                "MATCH (n:{table}) WHERE n.project = '{escaped}' \
                 RETURN n.id AS id, n.filePath AS file_path;"
            );
            let rows = self.storage.query(&cypher)?;
            for row in rows {
                if row.len() < 2 {
                    continue;
                }
                let id = row[0].as_str().unwrap_or_default().to_string();
                let file_path = row[1].as_str().unwrap_or_default().to_string();
                id_to_path.insert(id, file_path.clone());
                let module_name = module_name_from_path(&file_path);
                module_files
                    .entry(module_name)
                    .or_default()
                    .push(file_path);
            }
        }
        // Deduplicate file paths within each module.
        for files in module_files.values_mut() {
            files.sort();
            files.dedup();
        }

        // (b) Load all CALLS edges → source, target.
        let calls_cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type = 'CALLS' AND e.project = '{escaped}' \
             RETURN e.source AS source, e.target AS target;"
        );
        let calls_rows = self.storage.query(&calls_cypher)?;

        // (c) Count internal / incoming / outgoing per module.
        let mut internal: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        let mut incoming: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        let mut outgoing: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        for row in calls_rows {
            if row.len() < 2 {
                continue;
            }
            let source_id = row[0].as_str().unwrap_or_default().to_string();
            let target_id = row[1].as_str().unwrap_or_default().to_string();
            let source_path = match id_to_path.get(&source_id) {
                Some(p) => p,
                None => continue,
            };
            let target_path = match id_to_path.get(&target_id) {
                Some(p) => p,
                None => continue,
            };
            let source_module = module_name_from_path(source_path);
            let target_module = module_name_from_path(target_path);
            if source_module == target_module {
                *internal.entry(source_module).or_insert(0) += 1;
            } else {
                *outgoing.entry(source_module).or_insert(0) += 1;
                *incoming.entry(target_module).or_insert(0) += 1;
            }
        }

        // (d) Build result: cohesion = internal / (internal + incoming + outgoing).
        let mut result: Vec<ModuleBoundary> = module_files
            .into_iter()
            .map(|(module_name, members)| {
                let internal_count = *internal.get(&module_name).unwrap_or(&0);
                let incoming_count = *incoming.get(&module_name).unwrap_or(&0);
                let outgoing_count = *outgoing.get(&module_name).unwrap_or(&0);
                let total = internal_count + incoming_count + outgoing_count;
                let cohesion = if total == 0 {
                    1.0
                } else {
                    f64::from(internal_count) / f64::from(total)
                };
                ModuleBoundary {
                    module_name,
                    members,
                    incoming_deps: incoming_count,
                    outgoing_deps: outgoing_count,
                    cohesion,
                }
            })
            .collect();
        result.sort_by(|a, b| a.module_name.cmp(&b.module_name));
        Ok(result)
    }

    /// Analyzes dependency directions between modules and detects circular
    /// dependencies via DFS coloring.
    ///
    /// # Errors
    ///
    /// Returns [`crate::storage::error::StorageError`] if any Cypher query
    /// fails.
    fn analyze_dependency_directions(
        &self,
        project: &str,
    ) -> StorageResult<Vec<DepDirection>> {
        let escaped = escape_cypher_string(project);

        // (a) Load Function + Method nodes → id → filePath.
        let mut id_to_path: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for table in &["Function", "Method"] {
            let cypher = format!(
                "MATCH (n:{table}) WHERE n.project = '{escaped}' \
                 RETURN n.id AS id, n.filePath AS file_path;"
            );
            let rows = self.storage.query(&cypher)?;
            for row in rows {
                if row.len() < 2 {
                    continue;
                }
                let id = row[0].as_str().unwrap_or_default().to_string();
                let file_path = row[1].as_str().unwrap_or_default().to_string();
                id_to_path.insert(id, file_path);
            }
        }

        // (b) Load CALLS edges → build module adjacency list + unique edge set.
        let calls_cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type = 'CALLS' AND e.project = '{escaped}' \
             RETURN e.source AS source, e.target AS target;"
        );
        let calls_rows = self.storage.query(&calls_cypher)?;
        let mut adj: std::collections::HashMap<String, std::collections::HashSet<String>> =
            std::collections::HashMap::new();
        let mut edges: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        for row in calls_rows {
            if row.len() < 2 {
                continue;
            }
            let source_id = row[0].as_str().unwrap_or_default().to_string();
            let target_id = row[1].as_str().unwrap_or_default().to_string();
            let source_path = match id_to_path.get(&source_id) {
                Some(p) => p,
                None => continue,
            };
            let target_path = match id_to_path.get(&target_id) {
                Some(p) => p,
                None => continue,
            };
            let source_module = module_name_from_path(source_path);
            let target_module = module_name_from_path(target_path);
            if source_module == target_module {
                continue;
            }
            adj.entry(source_module.clone())
                .or_default()
                .insert(target_module.clone());
            edges.insert((source_module, target_module));
        }

        // (c) For each edge (from, to), check if `to` can reach `from` (cycle).
        let mut result: Vec<DepDirection> = edges
            .into_iter()
            .map(|(from, to)| {
                let is_circular = can_reach(&adj, &to, &from);
                DepDirection {
                    from_module: from,
                    to_module: to,
                    is_circular,
                }
            })
            .collect();
        result.sort_by(|a, b| {
            a.from_module
                .cmp(&b.from_module)
                .then_with(|| a.to_module.cmp(&b.to_module))
        });
        Ok(result)
    }

    /// Detects architectural layers (Controller/Service/Repository/Model) by
    /// classifying functions based on their edge patterns.
    ///
    /// # Errors
    ///
    /// Returns [`crate::storage::error::StorageError`] if any Cypher query
    /// fails.
    fn detect_layers(&self, project: &str) -> StorageResult<Vec<LayerInfo>> {
        let escaped = escape_cypher_string(project);

        // (a) Load Function + Method nodes → id → qualifiedName.
        let mut func_id_to_qn: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for table in &["Function", "Method"] {
            let cypher = format!(
                "MATCH (n:{table}) WHERE n.project = '{escaped}' \
                 RETURN n.id AS id, n.qualifiedName AS qualified_name;"
            );
            let rows = self.storage.query(&cypher)?;
            for row in rows {
                if row.len() < 2 {
                    continue;
                }
                let id = row[0].as_str().unwrap_or_default().to_string();
                let qn = row[1].as_str().unwrap_or_default().to_string();
                func_id_to_qn.insert(id, qn);
            }
        }

        // (b) Load type nodes (Class/Struct/Enum/Trait/Interface) → id → qualifiedName.
        let mut type_id_to_qn: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for table in &["Class", "Struct", "Enum", "Trait", "Interface"] {
            let cypher = format!(
                "MATCH (n:{table}) WHERE n.project = '{escaped}' \
                 RETURN n.id AS id, n.qualifiedName AS qualified_name;"
            );
            let rows = self.storage.query(&cypher)?;
            for row in rows {
                if row.len() < 2 {
                    continue;
                }
                let id = row[0].as_str().unwrap_or_default().to_string();
                let qn = row[1].as_str().unwrap_or_default().to_string();
                type_id_to_qn.insert(id, qn);
            }
        }

        // (c) Load HANDLES_ROUTE edges → controller_ids (source ids).
        let handles_cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type = 'HANDLES_ROUTE' AND e.project = '{escaped}' \
             RETURN e.source AS source;"
        );
        let handles_rows = self.storage.query(&handles_cypher)?;
        let mut controller_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for row in handles_rows {
            if let Some(source) = row.first().and_then(|v| v.as_str()) {
                controller_ids.insert(source.to_string());
            }
        }

        // (d) Load CALLS edges → calls_from (source → targets) + all_calls_ids.
        let calls_cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type = 'CALLS' AND e.project = '{escaped}' \
             RETURN e.source AS source, e.target AS target;"
        );
        let calls_rows = self.storage.query(&calls_cypher)?;
        let mut calls_from: std::collections::HashMap<String, std::collections::HashSet<String>> =
            std::collections::HashMap::new();
        let mut all_calls_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for row in calls_rows {
            if row.len() < 2 {
                continue;
            }
            let source = row[0].as_str().unwrap_or_default().to_string();
            let target = row[1].as_str().unwrap_or_default().to_string();
            calls_from
                .entry(source.clone())
                .or_default()
                .insert(target.clone());
            all_calls_ids.insert(source);
            all_calls_ids.insert(target);
        }

        // (e) Load FETCHES edges → repository_ids (source ids).
        let fetches_cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type = 'FETCHES' AND e.project = '{escaped}' \
             RETURN e.source AS source;"
        );
        let fetches_rows = self.storage.query(&fetches_cypher)?;
        let mut repository_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for row in fetches_rows {
            if let Some(source) = row.first().and_then(|v| v.as_str()) {
                repository_ids.insert(source.to_string());
            }
        }

        // (f) Load HAS_PROPERTY edges → model_candidate_ids (source ids).
        let has_prop_cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type = 'HAS_PROPERTY' AND e.project = '{escaped}' \
             RETURN e.source AS source;"
        );
        let has_prop_rows = self.storage.query(&has_prop_cypher)?;
        let mut model_candidate_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for row in has_prop_rows {
            if let Some(source) = row.first().and_then(|v| v.as_str()) {
                model_candidate_ids.insert(source.to_string());
            }
        }

        // (g) Compute functions called by controllers (Service candidates).
        let mut called_by_controllers: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for controller_id in &controller_ids {
            if let Some(targets) = calls_from.get(controller_id) {
                for target in targets {
                    called_by_controllers.insert(target.clone());
                }
            }
        }

        // (h) Classify functions: Controller > Service > Repository.
        let mut controllers: Vec<String> = Vec::new();
        let mut services: Vec<String> = Vec::new();
        let mut repositories: Vec<String> = Vec::new();
        let mut models: Vec<String> = Vec::new();
        for (id, qn) in &func_id_to_qn {
            if controller_ids.contains(id) {
                controllers.push(qn.clone());
            } else if called_by_controllers.contains(id) {
                services.push(qn.clone());
            } else if repository_ids.contains(id) {
                repositories.push(qn.clone());
            }
        }

        // (i) Classify type nodes: Model = HAS_PROPERTY source + no CALLS edges.
        for (id, qn) in &type_id_to_qn {
            if model_candidate_ids.contains(id) && !all_calls_ids.contains(id) {
                models.push(qn.clone());
            }
        }

        // (j) Sort members and build result (only non-empty layers).
        controllers.sort();
        services.sort();
        repositories.sort();
        models.sort();
        let mut result = Vec::new();
        if !controllers.is_empty() {
            result.push(LayerInfo {
                layer: "Controller".to_string(),
                members: controllers,
            });
        }
        if !services.is_empty() {
            result.push(LayerInfo {
                layer: "Service".to_string(),
                members: services,
            });
        }
        if !repositories.is_empty() {
            result.push(LayerInfo {
                layer: "Repository".to_string(),
                members: repositories,
            });
        }
        if !models.is_empty() {
            result.push(LayerInfo {
                layer: "Model".to_string(),
                members: models,
            });
        }
        Ok(result)
    }

    /// Loads cross-service dependencies by running the multi-protocol
    /// [`CrossServiceDetector`] and converting each match into a
    /// [`CrossServiceDep`]. The caller id becomes `from_module`, the
    /// callee becomes `to_module`, and the protocol is stringified via
    /// [`protocol_to_string`].
    ///
    /// # Errors
    ///
    /// Returns [`crate::storage::error::StorageError`] if any underlying
    /// Cypher query fails.
    fn load_cross_service_deps(&self, project: &str) -> StorageResult<Vec<CrossServiceDep>> {
        let detector = CrossServiceDetector::new(self.storage);
        let matches = detector.detect_all(project)?;
        let deps = matches
            .into_iter()
            .map(|m| CrossServiceDep {
                from_module: m.caller,
                to_module: m.callee,
                protocol: protocol_to_string(&m.protocol),
            })
            .collect();
        Ok(deps)
    }
}

/// Maps a [`ServiceProtocol`] to its canonical string label for
/// [`CrossServiceDep::protocol`]. Deterministic mapping (Rule 5) — no
/// `Display` impl dependency.
fn protocol_to_string(protocol: &ServiceProtocol) -> String {
    match protocol {
        ServiceProtocol::HttpRest => "HTTP".to_string(),
        ServiceProtocol::Grpc => "gRPC".to_string(),
        ServiceProtocol::GraphQL => "GraphQL".to_string(),
        ServiceProtocol::MessageQueue => "MessageQueue".to_string(),
        ServiceProtocol::EventBus => "EventBus".to_string(),
    }
}

/// Extracts the package prefix from a qualified name.
///
/// Returns the first 2 components joined by `.`, or `None` if the qualified
/// name has fewer than 2 components.
///
/// # Examples
///
/// ```ignore
/// // `package_prefix` is crate-private; see unit tests for verified behavior.
/// # use codenexus::analysis::architecture::package_prefix;
/// assert_eq!(package_prefix("com.example.Foo"), Some("com.example"));
/// assert_eq!(package_prefix("demo.foo"), Some("demo.foo"));
/// assert_eq!(package_prefix("foo"), None);
/// ```
fn package_prefix(qualified_name: &str) -> Option<&str> {
    let components: Vec<&str> = qualified_name.split('.').collect();
    if components.len() < 2 {
        return None;
    }
    // Take first 2 components.
    let end = components[0].len() + 1 + components[1].len();
    Some(&qualified_name[..end])
}

/// Extracts the module name (parent directory) from a file path.
///
/// `/src/a/foo.rs` → `/src/a`; `foo.rs` → ``.
fn module_name_from_path(file_path: &str) -> String {
    std::path::Path::new(file_path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Returns `true` if `target` is reachable from `start` in the adjacency list
/// via DFS. Used for circular-dependency detection: if `to` can reach `from`,
/// then the edge `(from, to)` participates in a cycle.
fn can_reach(
    adj: &std::collections::HashMap<String, std::collections::HashSet<String>>,
    start: &str,
    target: &str,
) -> bool {
    if start == target {
        return true;
    }
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut stack: Vec<String> = vec![start.to_string()];
    while let Some(node) = stack.pop() {
        if !visited.insert(node.clone()) {
            continue;
        }
        if let Some(neighbors) = adj.get(&node) {
            for n in neighbors {
                if n == target {
                    return true;
                }
                if !visited.contains(n) {
                    stack.push(n.clone());
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, StorageModule};
    use tempfile::TempDir;

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("arch_testdb");
        std::mem::forget(dir);
        path
    }

    /// Builds a Kit backed by an on-disk database at `db`.
    fn build_kit_for_db(db: &std::path::Path) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    /// Returns the `dyn Storage` capability from `kit`.
    fn storage(kit: &AsyncKit<AsyncReady>) -> std::sync::Arc<dyn crate::storage::capability::Storage> {
        kit.require::<StorageModule>().expect("require_storage")
    }

    /// Creates a File node.
    fn create_file(kit: &AsyncKit<AsyncReady>, id: &str, project: &str, file_path: &str, language: &str) {
        let storage = storage(kit);
        let cypher = format!(
            "CREATE (:File {{id: '{}', project: '{}', name: '{}', filePath: '{}', \
             language: '{}', hash: '', lineCount: 0}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(file_path.split('/').next_back().unwrap_or("file")),
            escape_cypher_string(file_path),
            escape_cypher_string(language),
        );
        storage.execute(&cypher).expect("create file");
    }

    /// Creates a Function node.
    fn create_function(
        kit: &AsyncKit<AsyncReady>,
        id: &str,
        project: &str,
        name: &str,
        qn: &str,
        file: &str,
        line: u32,
    ) {
        let storage = storage(kit);
        let end_line = line + 10;
        let cypher = format!(
            "CREATE (:Function {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '{}', startLine: {}, endLine: {}, signature: '', returnType: '', \
             isExported: false, docstring: '', content: '', parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(name),
            escape_cypher_string(qn),
            escape_cypher_string(file),
            line,
            end_line,
        );
        storage.execute(&cypher).expect("create function");
    }

    /// Creates a Function node with source `content` (for cross-service
    /// detection tests that need string literals in the function body).
    #[allow(clippy::too_many_arguments)]
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
        let storage = storage(kit);
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
        storage.execute(&cypher).expect("create function with content");
    }

    /// Creates a Method node.
    fn create_method(
        kit: &AsyncKit<AsyncReady>,
        id: &str,
        project: &str,
        name: &str,
        qn: &str,
        file: &str,
        line: u32,
    ) {
        let storage = storage(kit);
        let end_line = line + 10;
        let cypher = format!(
            "CREATE (:Method {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '{}', startLine: {}, endLine: {}, signature: '', returnType: '', \
             isExported: false, docstring: '', content: '', parameterCount: 0, parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(name),
            escape_cypher_string(qn),
            escape_cypher_string(file),
            line,
            end_line,
        );
        storage.execute(&cypher).expect("create method");
    }

    /// Creates a Class node.
    fn create_class(
        kit: &AsyncKit<AsyncReady>,
        id: &str,
        project: &str,
        name: &str,
        qn: &str,
        file: &str,
        line: u32,
    ) {
        let storage = storage(kit);
        let end_line = line + 10;
        let cypher = format!(
            "CREATE (:Class {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '{}', startLine: {}, endLine: {}, isExported: false, docstring: '', \
             content: '', parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(name),
            escape_cypher_string(qn),
            escape_cypher_string(file),
            line,
            end_line,
        );
        storage.execute(&cypher).expect("create class");
    }

    /// Creates a Route node.
    fn create_route(kit: &AsyncKit<AsyncReady>, id: &str, project: &str, path: &str, method: &str) {
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

    /// Creates a Handler node.
    fn create_handler(kit: &AsyncKit<AsyncReady>, id: &str, project: &str, name: &str) {
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

    /// Creates a CodeRelation edge.
    fn create_edge(
        kit: &AsyncKit<AsyncReady>,
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

    // --- package_prefix unit tests ---

    #[test]
    fn package_prefix_extracts_first_two_components() {
        assert_eq!(package_prefix("com.example.Foo"), Some("com.example"));
        assert_eq!(package_prefix("com.example.Bar"), Some("com.example"));
    }

    #[test]
    fn package_prefix_two_components_returns_both() {
        assert_eq!(package_prefix("demo.foo"), Some("demo.foo"));
    }

    #[test]
    fn package_prefix_single_component_returns_none() {
        assert_eq!(package_prefix("foo"), None);
    }

    #[test]
    fn package_prefix_empty_returns_none() {
        assert_eq!(package_prefix(""), None);
    }

    // --- ArchitectureAnalyzer tests ---

    #[test]
    fn overview_returns_empty_for_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        assert!(result.languages.is_empty(), "languages should be empty");
        assert!(result.packages.is_empty(), "packages should be empty");
        assert!(
            result.entry_points.is_empty(),
            "entry_points should be empty"
        );
        assert!(result.routes.is_empty(), "routes should be empty");
        assert!(result.hotspots.is_empty(), "hotspots should be empty");
    }

    #[test]
    fn overview_counts_languages() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_file(&kit, "f1", "demo", "/src/main.rs", "rust");
        create_file(&kit, "f2", "demo", "/src/app.py", "python");

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        assert_eq!(result.languages.len(), 2, "should have 2 languages");
        // Languages are sorted alphabetically.
        let py = result
            .languages
            .iter()
            .find(|l| l.language == "python")
            .expect("python should be present");
        assert_eq!(py.file_count, 1, "python file_count should be 1");
        let rs = result
            .languages
            .iter()
            .find(|l| l.language == "rust")
            .expect("rust should be present");
        assert_eq!(rs.file_count, 1, "rust file_count should be 1");
    }

    #[test]
    fn overview_counts_symbols_per_language() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_file(&kit, "f1", "demo", "/src/main.rs", "rust");
        create_file(&kit, "f2", "demo", "/src/app.py", "python");
        // 2 Rust functions + 1 Python function.
        create_function(&kit, "fn1", "demo", "foo", "demo.foo", "/src/main.rs", 1);
        create_function(&kit, "fn2", "demo", "bar", "demo.bar", "/src/main.rs", 10);
        create_function(&kit, "fn3", "demo", "baz", "demo.baz", "/src/app.py", 1);

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        let rs = result
            .languages
            .iter()
            .find(|l| l.language == "rust")
            .expect("rust should be present");
        assert_eq!(rs.symbol_count, 2, "rust symbol_count should be 2");
        let py = result
            .languages
            .iter()
            .find(|l| l.language == "python")
            .expect("python should be present");
        assert_eq!(py.symbol_count, 1, "python symbol_count should be 1");
    }

    #[test]
    fn overview_lists_entry_points() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(
            &kit,
            "f_main",
            "demo",
            "main",
            "demo.main",
            "/src/main.rs",
            1,
        );
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        assert_eq!(result.entry_points.len(), 1, "should have 1 entry point");
        let ep = &result.entry_points[0];
        assert_eq!(ep.name, "main");
        assert_eq!(ep.qualified_name, "demo.main");
        assert_eq!(ep.file_path, "/src/main.rs");
        assert_eq!(ep.line, 1);
    }

    #[test]
    fn overview_lists_entry_points_multiple_patterns() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f1", "demo", "main", "demo.main", "/src/main.rs", 1);
        create_function(&kit, "f2", "demo", "Main", "demo.Main", "/src/Main.cs", 1);
        create_function(
            &kit,
            "f3",
            "demo",
            "__main__",
            "demo.__main__",
            "/src/app.py",
            1,
        );
        create_function(&kit, "f4", "demo", "foo", "demo.foo", "/src/lib.rs", 1);

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        assert_eq!(result.entry_points.len(), 3, "should have 3 entry points");
        let names: Vec<&str> = result
            .entry_points
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert!(names.contains(&"main"));
        assert!(names.contains(&"Main"));
        assert!(names.contains(&"__main__"));
        assert!(!names.contains(&"foo"));
    }

    #[test]
    fn overview_lists_routes() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_route(&kit, "r1", "demo", "/api/users", "GET");
        create_handler(&kit, "h1", "demo", "list_users");
        create_edge(&kit, "e1", "h1", "r1", "HANDLES", "demo");

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        assert_eq!(result.routes.len(), 1, "should have 1 route");
        let route = &result.routes[0];
        assert_eq!(route.path, "/api/users");
        assert_eq!(route.method, "GET");
        assert_eq!(route.handler, "list_users");
    }

    #[test]
    fn overview_route_without_handler_has_empty_handler() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_route(&kit, "r1", "demo", "/api/orphan", "POST");

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        assert_eq!(result.routes.len(), 1, "should have 1 route");
        assert_eq!(result.routes[0].handler, "", "handler should be empty");
    }

    #[test]
    fn overview_identifies_hotspots() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
        create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/a.rs", 1);
        create_function(&kit, "f_b", "demo", "b", "demo.b", "/src/b.rs", 1);
        create_function(&kit, "f_c", "demo", "c", "demo.c", "/src/c.rs", 1);
        // 3 CALLS edges pointing to foo.
        create_edge(&kit, "e1", "f_a", "f_foo", "CALLS", "demo");
        create_edge(&kit, "e2", "f_b", "f_foo", "CALLS", "demo");
        create_edge(&kit, "e3", "f_c", "f_foo", "CALLS", "demo");

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        assert!(!result.hotspots.is_empty(), "should have hotspots");
        let foo = result
            .hotspots
            .iter()
            .find(|h| h.name == "foo")
            .expect("foo should be a hotspot");
        assert_eq!(foo.caller_count, 3, "foo caller_count should be 3");
        assert_eq!(foo.qualified_name, "demo.foo");
    }

    #[test]
    fn overview_hotspots_sorted_by_caller_count_desc() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
        create_function(&kit, "f_bar", "demo", "bar", "demo.bar", "/src/lib.rs", 10);
        create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/a.rs", 1);
        create_function(&kit, "f_b", "demo", "b", "demo.b", "/src/b.rs", 1);
        create_function(&kit, "f_c", "demo", "c", "demo.c", "/src/c.rs", 1);
        // foo has 3 callers, bar has 1 caller.
        create_edge(&kit, "e1", "f_a", "f_foo", "CALLS", "demo");
        create_edge(&kit, "e2", "f_b", "f_foo", "CALLS", "demo");
        create_edge(&kit, "e3", "f_c", "f_foo", "CALLS", "demo");
        create_edge(&kit, "e4", "f_a", "f_bar", "CALLS", "demo");

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        assert!(
            result.hotspots.len() >= 2,
            "should have at least 2 hotspots"
        );
        // First hotspot should be foo (3 callers).
        assert_eq!(result.hotspots[0].name, "foo");
        assert_eq!(result.hotspots[0].caller_count, 3);
        // Second should be bar (1 caller).
        assert_eq!(result.hotspots[1].name, "bar");
        assert_eq!(result.hotspots[1].caller_count, 1);
    }

    #[test]
    fn overview_hotspots_limited_to_10() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Create 15 functions, each with 1 caller.
        for i in 0..15 {
            let id = format!("f_{i}");
            let name = format!("func{i}");
            let qn = format!("demo.func{i}");
            let file = format!("/src/f{i}.rs");
            create_function(&kit, &id, "demo", &name, &qn, &file, 1);
        }
        // Create a caller for each (self-call to keep it simple).
        for i in 0..15 {
            let id = format!("f_{i}");
            let edge_id = format!("e_{i}");
            create_edge(&kit, &edge_id, &id, &id, "CALLS", "demo");
        }

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        assert_eq!(
            result.hotspots.len(),
            10,
            "hotspots should be limited to 10"
        );
    }

    #[test]
    fn overview_groups_packages() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_class(
            &kit,
            "c1",
            "demo",
            "Foo",
            "com.example.Foo",
            "/src/Foo.java",
            1,
        );
        create_class(
            &kit,
            "c2",
            "demo",
            "Bar",
            "com.example.Bar",
            "/src/Bar.java",
            1,
        );
        create_class(
            &kit,
            "c3",
            "demo",
            "Baz",
            "org.other.Baz",
            "/src/Baz.java",
            1,
        );

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        let pkg = result
            .packages
            .iter()
            .find(|p| p.package == "com.example")
            .expect("com.example package should exist");
        assert_eq!(pkg.symbol_count, 2, "com.example should have 2 symbols");
        let other = result
            .packages
            .iter()
            .find(|p| p.package == "org.other")
            .expect("org.other package should exist");
        assert_eq!(other.symbol_count, 1, "org.other should have 1 symbol");
    }

    #[test]
    fn overview_includes_methods_in_packages() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_method(
            &kit,
            "m1",
            "demo",
            "helper",
            "com.example.helper",
            "/src/lib.rs",
            1,
        );
        create_function(
            &kit,
            "f1",
            "demo",
            "foo",
            "com.example.foo",
            "/src/lib.rs",
            10,
        );

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        let pkg = result
            .packages
            .iter()
            .find(|p| p.package == "com.example")
            .expect("com.example package should exist");
        assert_eq!(pkg.symbol_count, 2, "should count both method and function");
    }

    #[test]
    fn overview_filters_by_project() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_file(&kit, "f1", "demo", "/src/main.rs", "rust");
        create_file(&kit, "f2", "other", "/src/app.py", "python");

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        assert_eq!(
            result.languages.len(),
            1,
            "should only see demo's languages"
        );
        assert_eq!(result.languages[0].language, "rust");
    }

    #[test]
    fn overview_hotspot_method_nodes() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_method(
            &kit,
            "m1",
            "demo",
            "helper",
            "demo.Class.helper",
            "/src/lib.rs",
            5,
        );
        create_function(&kit, "f1", "demo", "a", "demo.a", "/src/a.rs", 1);
        create_edge(&kit, "e1", "f1", "m1", "CALLS", "demo");

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        let helper = result
            .hotspots
            .iter()
            .find(|h| h.name == "helper")
            .expect("helper method should be a hotspot");
        assert_eq!(helper.caller_count, 1);
        assert_eq!(helper.qualified_name, "demo.Class.helper");
    }

    // --- T028: multi-dimensional architecture type tests ---

    #[test]
    fn module_boundary_serializes_all_fields() {
        let mb = ModuleBoundary {
            module_name: "src/api".to_string(),
            members: vec!["src/api/handler.rs".to_string(), "src/api/route.rs".to_string()],
            incoming_deps: 3,
            outgoing_deps: 2,
            cohesion: 0.71,
        };
        let json = serde_json::to_string(&mb).expect("serialize");
        assert!(json.contains("\"module_name\":\"src/api\""), "json: {json}");
        assert!(json.contains("\"members\""), "json: {json}");
        assert!(json.contains("\"incoming_deps\":3"), "json: {json}");
        assert!(json.contains("\"outgoing_deps\":2"), "json: {json}");
        assert!(json.contains("\"cohesion\":0.71"), "json: {json}");
    }

    #[test]
    fn dep_direction_serializes_all_fields() {
        let dd = DepDirection {
            from_module: "module_a".to_string(),
            to_module: "module_b".to_string(),
            is_circular: true,
        };
        let json = serde_json::to_string(&dd).expect("serialize");
        assert!(json.contains("\"from_module\":\"module_a\""), "json: {json}");
        assert!(json.contains("\"to_module\":\"module_b\""), "json: {json}");
        assert!(json.contains("\"is_circular\":true"), "json: {json}");
    }

    #[test]
    fn layer_info_serializes_all_fields() {
        let li = LayerInfo {
            layer: "Controller".to_string(),
            members: vec!["demo.handler".to_string()],
        };
        let json = serde_json::to_string(&li).expect("serialize");
        assert!(json.contains("\"layer\":\"Controller\""), "json: {json}");
        assert!(json.contains("\"members\""), "json: {json}");
        assert!(json.contains("\"demo.handler\""), "json: {json}");
    }

    #[test]
    fn cross_service_dep_serializes_all_fields() {
        let csd = CrossServiceDep {
            from_module: "svc_a".to_string(),
            to_module: "svc_b".to_string(),
            protocol: "HTTP".to_string(),
        };
        let json = serde_json::to_string(&csd).expect("serialize");
        assert!(json.contains("\"from_module\":\"svc_a\""), "json: {json}");
        assert!(json.contains("\"to_module\":\"svc_b\""), "json: {json}");
        assert!(json.contains("\"protocol\":\"HTTP\""), "json: {json}");
    }

    #[test]
    fn overview_includes_new_fields_in_json() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        let json = serde_json::to_string(&result).expect("serialize");
        assert!(
            json.contains("\"module_boundaries\""),
            "json should contain module_boundaries: {json}"
        );
        assert!(
            json.contains("\"dependency_directions\""),
            "json should contain dependency_directions: {json}"
        );
        assert!(
            json.contains("\"layers\""),
            "json should contain layers: {json}"
        );
        assert!(
            json.contains("\"cross_service_deps\""),
            "json should contain cross_service_deps: {json}"
        );
    }

    #[test]
    fn overview_cross_service_deps_populated_when_matches_exist() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_route(&kit, "r1", "demo", "/api/users", "GET");
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

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        assert!(
            !result.cross_service_deps.is_empty(),
            "cross_service_deps should be populated when matches exist: {:?}",
            result.cross_service_deps
        );
        let dep = &result.cross_service_deps[0];
        assert_eq!(dep.from_module, "f1", "from_module should be caller id");
        assert_eq!(dep.to_module, "r1", "to_module should be callee (route id)");
        assert_eq!(dep.protocol, "HTTP", "protocol should be HTTP for REST match");
    }

    #[test]
    fn overview_cross_service_deps_empty_for_no_matches() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f1", "demo", "foo", "demo.foo", "/src/a.rs", 1);

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        assert!(
            result.cross_service_deps.is_empty(),
            "cross_service_deps should be empty when no routes/callers match: {:?}",
            result.cross_service_deps
        );
    }

    // --- T029: module boundary detection tests ---

    #[test]
    fn detect_module_boundaries_cohesion_5_internal_2_external() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Module A: /src/a/
        create_function(&kit, "a1", "demo", "a1", "demo.a1", "/src/a/a1.rs", 1);
        create_function(&kit, "a2", "demo", "a2", "demo.a2", "/src/a/a2.rs", 1);
        create_function(&kit, "a3", "demo", "a3", "demo.a3", "/src/a/a3.rs", 1);
        // Module B: /src/b/
        create_function(&kit, "b1", "demo", "b1", "demo.b1", "/src/b/b1.rs", 1);
        create_function(&kit, "b2", "demo", "b2", "demo.b2", "/src/b/b2.rs", 1);

        // 5 internal CALLS edges within module A.
        create_edge(&kit, "e1", "a1", "a2", "CALLS", "demo");
        create_edge(&kit, "e2", "a2", "a3", "CALLS", "demo");
        create_edge(&kit, "e3", "a1", "a3", "CALLS", "demo");
        create_edge(&kit, "e4", "a3", "a1", "CALLS", "demo");
        create_edge(&kit, "e5", "a2", "a1", "CALLS", "demo");
        // 2 external CALLS edges between A and B.
        create_edge(&kit, "e6", "a1", "b1", "CALLS", "demo");
        create_edge(&kit, "e7", "b2", "a1", "CALLS", "demo");

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        let module_a = result
            .module_boundaries
            .iter()
            .find(|m| m.module_name.ends_with("src/a"))
            .expect("module A should exist");
        assert_eq!(module_a.incoming_deps, 1, "incoming_deps for A");
        assert_eq!(module_a.outgoing_deps, 1, "outgoing_deps for A");
        let expected = 5.0 / 7.0;
        assert!(
            (module_a.cohesion - expected).abs() < 0.001,
            "cohesion should be ~{expected:.3}, got {}",
            module_a.cohesion
        );
    }

    #[test]
    fn detect_module_boundaries_no_external_deps_cohesion_1() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "a1", "demo", "a1", "demo.a1", "/src/a/a1.rs", 1);
        create_function(&kit, "a2", "demo", "a2", "demo.a2", "/src/a/a2.rs", 1);
        // Only internal edges.
        create_edge(&kit, "e1", "a1", "a2", "CALLS", "demo");
        create_edge(&kit, "e2", "a2", "a1", "CALLS", "demo");

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        let module_a = result
            .module_boundaries
            .iter()
            .find(|m| m.module_name.ends_with("src/a"))
            .expect("module A should exist");
        assert_eq!(module_a.incoming_deps, 0, "no incoming deps");
        assert_eq!(module_a.outgoing_deps, 0, "no outgoing deps");
        assert!(
            (module_a.cohesion - 1.0).abs() < 0.001,
            "cohesion should be 1.0, got {}",
            module_a.cohesion
        );
    }

    // --- T030: dependency direction analysis tests ---

    #[test]
    fn analyze_dependency_directions_detects_circular() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "a1", "demo", "a1", "demo.a1", "/src/a/a1.rs", 1);
        create_function(&kit, "b1", "demo", "b1", "demo.b1", "/src/b/b1.rs", 1);
        // A→B and B→A (circular).
        create_edge(&kit, "e1", "a1", "b1", "CALLS", "demo");
        create_edge(&kit, "e2", "b1", "a1", "CALLS", "demo");

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        assert_eq!(
            result.dependency_directions.len(),
            2,
            "should have 2 directions"
        );
        for dd in &result.dependency_directions {
            assert!(
                dd.is_circular,
                "edge {}→{} should be circular",
                dd.from_module,
                dd.to_module
            );
        }
    }

    #[test]
    fn analyze_dependency_directions_no_circular() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "a1", "demo", "a1", "demo.a1", "/src/a/a1.rs", 1);
        create_function(&kit, "b1", "demo", "b1", "demo.b1", "/src/b/b1.rs", 1);
        create_function(&kit, "c1", "demo", "c1", "demo.c1", "/src/c/c1.rs", 1);
        // A→B→C (no circular).
        create_edge(&kit, "e1", "a1", "b1", "CALLS", "demo");
        create_edge(&kit, "e2", "b1", "c1", "CALLS", "demo");

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        assert_eq!(
            result.dependency_directions.len(),
            2,
            "should have 2 directions"
        );
        for dd in &result.dependency_directions {
            assert!(
                !dd.is_circular,
                "edge {}→{} should not be circular",
                dd.from_module,
                dd.to_module
            );
        }
    }

    // --- T031: layer detection tests ---

    #[test]
    fn detect_layers_controller_handling_route() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Function that handles a route → Controller layer.
        create_function(
            &kit,
            "ctrl1",
            "demo",
            "list_users",
            "demo.list_users",
            "/src/api/handler.rs",
            1,
        );
        create_route(&kit, "r1", "demo", "/api/users", "GET");
        create_edge(&kit, "e1", "ctrl1", "r1", "HANDLES_ROUTE", "demo");

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        let controller = result
            .layers
            .iter()
            .find(|l| l.layer == "Controller")
            .expect("Controller layer should exist");
        assert!(
            controller.members.contains(&"demo.list_users".to_string()),
            "Controller members should contain demo.list_users, got: {:?}",
            controller.members
        );
    }

    // --- protocol_to_string unit tests (lines 895-898) ---

    #[test]
    fn protocol_to_string_http_rest_returns_http() {
        assert_eq!(protocol_to_string(&ServiceProtocol::HttpRest), "HTTP");
    }

    #[test]
    fn protocol_to_string_grpc_returns_grpc() {
        assert_eq!(protocol_to_string(&ServiceProtocol::Grpc), "gRPC");
    }

    #[test]
    fn protocol_to_string_graphql_returns_graphql() {
        assert_eq!(protocol_to_string(&ServiceProtocol::GraphQL), "GraphQL");
    }

    #[test]
    fn protocol_to_string_message_queue_returns_message_queue() {
        assert_eq!(
            protocol_to_string(&ServiceProtocol::MessageQueue),
            "MessageQueue"
        );
    }

    #[test]
    fn protocol_to_string_event_bus_returns_event_bus() {
        assert_eq!(protocol_to_string(&ServiceProtocol::EventBus), "EventBus");
    }

    // --- can_reach unit tests (line 945: start == target early return) ---

    #[test]
    fn can_reach_returns_true_when_start_equals_target() {
        let mut adj: std::collections::HashMap<String, std::collections::HashSet<String>> =
            std::collections::HashMap::new();
        let mut neighbors = std::collections::HashSet::new();
        neighbors.insert("b".to_string());
        adj.insert("a".to_string(), neighbors);
        assert!(
            can_reach(&adj, "a", "a"),
            "start == target should return true immediately"
        );
    }

    #[test]
    fn can_reach_returns_true_for_direct_neighbor() {
        let mut adj: std::collections::HashMap<String, std::collections::HashSet<String>> =
            std::collections::HashMap::new();
        let mut neighbors = std::collections::HashSet::new();
        neighbors.insert("b".to_string());
        adj.insert("a".to_string(), neighbors);
        assert!(can_reach(&adj, "a", "b"), "a→b direct edge");
    }

    #[test]
    fn can_reach_returns_true_for_transitive_path() {
        let mut adj: std::collections::HashMap<String, std::collections::HashSet<String>> =
            std::collections::HashMap::new();
        let mut a_neighbors = std::collections::HashSet::new();
        a_neighbors.insert("b".to_string());
        adj.insert("a".to_string(), a_neighbors);
        let mut b_neighbors = std::collections::HashSet::new();
        b_neighbors.insert("c".to_string());
        adj.insert("b".to_string(), b_neighbors);
        assert!(can_reach(&adj, "a", "c"), "a→b→c transitive path");
    }

    #[test]
    fn can_reach_returns_false_for_unreachable() {
        let mut adj: std::collections::HashMap<String, std::collections::HashSet<String>> =
            std::collections::HashMap::new();
        let mut a_neighbors = std::collections::HashSet::new();
        a_neighbors.insert("b".to_string());
        adj.insert("a".to_string(), a_neighbors);
        assert!(
            !can_reach(&adj, "a", "z"),
            "z is not reachable from a"
        );
    }

    #[test]
    fn can_reach_returns_false_for_empty_graph() {
        let adj: std::collections::HashMap<String, std::collections::HashSet<String>> =
            std::collections::HashMap::new();
        assert!(
            !can_reach(&adj, "a", "b"),
            "empty graph → nothing reachable"
        );
    }

    // --- detect_layers: Service / Repository / Model layer tests ---

    #[test]
    fn detect_layers_service_layer_function_called_by_controller() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Controller: handles a route.
        create_function(
            &kit,
            "ctrl1",
            "demo",
            "list_users",
            "demo.list_users",
            "/src/api/handler.rs",
            1,
        );
        create_route(&kit, "r1", "demo", "/api/users", "GET");
        create_edge(&kit, "e1", "ctrl1", "r1", "HANDLES_ROUTE", "demo");
        // Service: called by the controller.
        create_function(
            &kit,
            "svc1",
            "demo",
            "fetch_users",
            "demo.fetch_users",
            "/src/service/user_service.rs",
            1,
        );
        create_edge(&kit, "e2", "ctrl1", "svc1", "CALLS", "demo");

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        let service = result
            .layers
            .iter()
            .find(|l| l.layer == "Service")
            .expect("Service layer should exist");
        assert!(
            service.members.contains(&"demo.fetch_users".to_string()),
            "Service members should contain demo.fetch_users, got: {:?}",
            service.members
        );
    }

    #[test]
    fn detect_layers_repository_layer_function_with_fetches_edge() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Repository: function with a FETCHES edge (source).
        create_function(
            &kit,
            "repo1",
            "demo",
            "find_user",
            "demo.find_user",
            "/src/repo/user_repo.rs",
            1,
        );
        create_function(
            &kit,
            "model1",
            "demo",
            "user_data",
            "demo.user_data",
            "/src/model/user.rs",
            1,
        );
        create_edge(&kit, "e1", "repo1", "model1", "FETCHES", "demo");

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        let repository = result
            .layers
            .iter()
            .find(|l| l.layer == "Repository")
            .expect("Repository layer should exist");
        assert!(
            repository.members.contains(&"demo.find_user".to_string()),
            "Repository members should contain demo.find_user, got: {:?}",
            repository.members
        );
    }

    #[test]
    fn detect_layers_model_layer_type_with_has_property_and_no_calls() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Model: Class node with HAS_PROPERTY edge and no CALLS edges.
        create_class(
            &kit,
            "cls1",
            "demo",
            "User",
            "demo.User",
            "/src/model/user.rs",
            1,
        );
        create_function(
            &kit,
            "prop1",
            "demo",
            "name_field",
            "demo.name_field",
            "/src/model/user.rs",
            5,
        );
        create_edge(&kit, "e1", "cls1", "prop1", "HAS_PROPERTY", "demo");

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        let model = result
            .layers
            .iter()
            .find(|l| l.layer == "Model")
            .expect("Model layer should exist");
        assert!(
            model.members.contains(&"demo.User".to_string()),
            "Model members should contain demo.User, got: {:?}",
            model.members
        );
    }

    #[test]
    fn detect_layers_all_four_layers_populated() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Controller: handles route.
        create_function(
            &kit,
            "ctrl1",
            "demo",
            "list_users",
            "demo.list_users",
            "/src/api/handler.rs",
            1,
        );
        create_route(&kit, "r1", "demo", "/api/users", "GET");
        create_edge(&kit, "e1", "ctrl1", "r1", "HANDLES_ROUTE", "demo");
        // Service: called by controller.
        create_function(
            &kit,
            "svc1",
            "demo",
            "fetch_users",
            "demo.fetch_users",
            "/src/service/user_service.rs",
            1,
        );
        create_edge(&kit, "e2", "ctrl1", "svc1", "CALLS", "demo");
        // Repository: has FETCHES edge.
        create_function(
            &kit,
            "repo1",
            "demo",
            "find_user",
            "demo.find_user",
            "/src/repo/user_repo.rs",
            1,
        );
        create_function(
            &kit,
            "data1",
            "demo",
            "db_query",
            "demo.db_query",
            "/src/repo/db.rs",
            1,
        );
        create_edge(&kit, "e3", "repo1", "data1", "FETCHES", "demo");
        // Model: Class with HAS_PROPERTY and no CALLS.
        create_class(
            &kit,
            "cls1",
            "demo",
            "User",
            "demo.User",
            "/src/model/user.rs",
            1,
        );
        create_function(
            &kit,
            "prop1",
            "demo",
            "name_field",
            "demo.name_field",
            "/src/model/user.rs",
            5,
        );
        create_edge(&kit, "e4", "cls1", "prop1", "HAS_PROPERTY", "demo");

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        let layer_names: Vec<&str> = result.layers.iter().map(|l| l.layer.as_str()).collect();
        assert!(
            layer_names.contains(&"Controller"),
            "Controller layer should exist: {:?}",
            layer_names
        );
        assert!(
            layer_names.contains(&"Service"),
            "Service layer should exist: {:?}",
            layer_names
        );
        assert!(
            layer_names.contains(&"Repository"),
            "Repository layer should exist: {:?}",
            layer_names
        );
        assert!(
            layer_names.contains(&"Model"),
            "Model layer should exist: {:?}",
            layer_names
        );
    }

    #[test]
    fn detect_layers_empty_when_no_relevant_edges() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Plain function with no HANDLES_ROUTE, no CALLS, no FETCHES, no HAS_PROPERTY.
        create_function(
            &kit,
            "f1",
            "demo",
            "plain",
            "demo.plain",
            "/src/lib.rs",
            1,
        );

        let storage = storage(&kit);
        let analyzer = ArchitectureAnalyzer::new(&*storage);
        let result = analyzer.overview("demo").expect("overview");
        assert!(
            result.layers.is_empty(),
            "no relevant edges → no layers, got: {:?}",
            result.layers
        );
    }
}
