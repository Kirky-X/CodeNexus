// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

use serde::Serialize;

use crate::kit::{AsyncKit, AsyncReady, StorageModule, TraceModule};
use crate::model::Language;
use crate::service::error::CodeNexusError;
#[cfg(feature = "cli")]
use crate::service::error::{kit_not_initialized, to_api_error, wrap_error};
#[cfg(feature = "cli")]
use crate::service::project::resolve_project_id;
#[cfg(feature = "cli")]
use crate::service::runtime::kit;
use crate::trace::taint_rules::{match_node, BUILTIN_RULES, RULES_VERSION};
use crate::trace::TaintPathTracer;

#[cfg(feature = "cli")]
use sdforge::forge;
#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;

/// One matched taint path (source → sink).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaintFinding {
    pub source: String,
    pub sink: String,
    pub depth: usize,
    /// Node names along the path.
    pub nodes: Vec<String>,
    /// Edge types along the path.
    pub edge_types: Vec<String>,
}

/// JSON-serializable taint audit output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TaintAuditOutput {
    /// `manual` or `rules`.
    pub mode: String,
    /// Builtin rules version (rules mode only; `null` in manual mode).
    pub rules_version: Option<String>,
    pub sources_found: usize,
    pub sinks_found: usize,
    pub pairs_scanned: usize,
    pub paths: Vec<TaintFinding>,
    /// `true` when the pair cap cut off remaining combinations.
    pub truncated: bool,
    pub diagnostics: Vec<crate::diagnostics::Diagnostic>,
}

/// Converts a [`crate::trace::TracePath`] into a finding.
fn finding(source: &str, sink: &str, path: &crate::trace::TracePath) -> TaintFinding {
    TaintFinding {
        source: source.to_string(),
        sink: sink.to_string(),
        depth: path.depth,
        nodes: path.nodes.iter().map(|n| n.name.clone()).collect(),
        edge_types: path.edges.iter().map(|e| e.edge_type.clone()).collect(),
    }
}

/// Rules mode — candidate node lookup in the indexed graph.
///
/// Queries function-like labels for each language that has rules (or only
/// `filter` when given). Returns `(node_id, name, qualified_name, language)`.
fn candidate_nodes(
    kit: &AsyncKit<AsyncReady>,
    filter: Option<Language>,
) -> Result<Vec<(String, String, String, Language)>, CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let languages: Vec<Language> = match filter {
        Some(lang) => vec![lang],
        None => BUILTIN_RULES.iter().map(|(lang, _)| *lang).collect(),
    };
    // Language lives on File nodes (function tables carry no language
    // column), so resolve each file's language and join via filePath.
    let mut out = Vec::new();
    for lang in languages {
        let escaped = crate::storage::schema::escape_cypher_string(&lang.to_string());
        let file_rows = storage.query(&format!(
            "MATCH (n:File) WHERE n.language = '{escaped}' RETURN n.filePath AS filePath;"
        ))?;
        let lang_files: std::collections::HashSet<String> = file_rows
            .iter()
            .filter_map(|r| r.first().and_then(|v| v.as_str()))
            .map(String::from)
            .collect();
        for label in [
            crate::model::NodeLabel::Function,
            crate::model::NodeLabel::Method,
        ] {
            let table = crate::storage::schema::escape_identifier(label.table_name());
            let cypher = format!(
                "MATCH (n:{table}) \
                 RETURN n.id AS id, n.name AS name, n.qualifiedName AS qualifiedName, n.filePath AS filePath;"
            );
            for row in storage.query(&cypher)? {
                let id = row.first().and_then(|v| v.as_str()).unwrap_or_default();
                let name = row.get(1).and_then(|v| v.as_str()).unwrap_or_default();
                let qn = row.get(2).and_then(|v| v.as_str()).unwrap_or_default();
                let file_path = row.get(3).and_then(|v| v.as_str()).unwrap_or_default();
                if !id.is_empty() && !name.is_empty() && lang_files.contains(file_path) {
                    out.push((id.to_string(), name.to_string(), qn.to_string(), lang));
                }
            }
        }
    }
    Ok(out)
}

/// Core logic — audits taint paths against an injected Kit (testable core).
#[cfg_attr(not(feature = "cli"), allow(dead_code))]
pub fn run_taint(
    kit: &AsyncKit<AsyncReady>,
    source: &str,
    sink: &str,
    language_filter: &str,
    max_pairs: usize,
    depth: usize,
    project: &str,
) -> Result<TaintAuditOutput, CodeNexusError> {
    if !project.trim().is_empty() {
        let storage = kit.require::<StorageModule>()?;
        resolve_project_id(&*storage, project)?;
    }

    let manual = !source.trim().is_empty() || !sink.trim().is_empty();
    if manual {
        if source.trim().is_empty() || sink.trim().is_empty() {
            return Err(CodeNexusError::InvalidInput(
                "manual taint mode requires both --source and --sink".to_string(),
            ));
        }
        let trace_engine = kit.require::<TraceModule>()?;
        let (graph, _truncated) =
            trace_engine.load_graph(source, depth, crate::trace::MAX_SUBGRAPH_NODES)?;
        let src_id = find_start_node_id(&graph, source).ok_or_else(|| {
            CodeNexusError::NotFound(format!("source symbol not found: {source}"))
        })?;
        let sink_id = find_start_node_id(&graph, sink)
            .ok_or_else(|| CodeNexusError::NotFound(format!("sink symbol not found: {sink}")))?;
        let tracer = TaintPathTracer::new(&graph);
        let paths = tracer.trace_taint(&src_id, &sink_id, depth);
        return Ok(TaintAuditOutput {
            mode: "manual".to_string(),
            rules_version: None,
            sources_found: 1,
            sinks_found: 1,
            pairs_scanned: 1,
            paths: paths.iter().map(|p| finding(source, sink, p)).collect(),
            truncated: false,
            diagnostics: Vec::new(),
        });
    }

    // Rules mode.
    let filter = if language_filter.trim().is_empty() {
        None
    } else {
        Some(language_filter.trim().parse::<Language>().map_err(|_| {
            CodeNexusError::InvalidInput(format!("unknown language filter: {language_filter}"))
        })?)
    };
    let candidates = candidate_nodes(kit, filter)?;

    let trace_engine = kit.require::<TraceModule>()?;
    let mut sources_found = 0usize;
    let mut sinks_found = 0usize;
    let mut pairs_scanned = 0usize;
    let mut paths = Vec::new();
    let mut truncated = false;

    for (lang, rules) in BUILTIN_RULES {
        if filter.is_some_and(|f| f != *lang) {
            continue;
        }
        let lang_candidates: Vec<_> = candidates.iter().filter(|(_, _, _, l)| l == lang).collect();
        let mut sources: Vec<(String, String)> = Vec::new();
        let mut sinks: Vec<(String, String)> = Vec::new();
        for (id, name, qn, _) in &lang_candidates {
            if !match_node(name, qn, rules, rules.sources).is_empty() {
                sources.push((id.clone(), name.clone()));
            }
            if !match_node(name, qn, rules, rules.sinks).is_empty() {
                sinks.push((id.clone(), name.clone()));
            }
        }
        sources_found += sources.len();
        sinks_found += sinks.len();

        'pairs: for (_src_id, src_name) in &sources {
            // Load the subgraph around each source once; sinks must be
            // reachable in it for a path to exist.
            let (graph, _t) =
                match trace_engine.load_graph(src_name, depth, crate::trace::MAX_SUBGRAPH_NODES) {
                    Ok(x) => x,
                    Err(_) => continue,
                };
            let tracer = TaintPathTracer::new(&graph);
            for (sink_id, sink_name) in &sinks {
                if pairs_scanned >= max_pairs {
                    truncated = true;
                    break 'pairs;
                }
                pairs_scanned += 1;
                let Some(sink_resolved) = find_start_node_id(&graph, sink_name) else {
                    continue;
                };
                if sink_resolved == find_start_node_id(&graph, src_name).unwrap_or_default() {
                    continue;
                }
                let _ = sink_id;
                for p in tracer.trace_taint(
                    &find_start_node_id(&graph, src_name).unwrap(),
                    &sink_resolved,
                    depth,
                ) {
                    paths.push(finding(src_name, sink_name, &p));
                }
            }
        }
    }

    paths.sort_by(|a, b| a.source.cmp(&b.source).then_with(|| a.sink.cmp(&b.sink)));
    paths.dedup_by(|a, b| a.source == b.source && a.sink == b.sink && a.depth == b.depth);

    Ok(TaintAuditOutput {
        mode: "rules".to_string(),
        rules_version: Some(RULES_VERSION.to_string()),
        sources_found,
        sinks_found,
        pairs_scanned,
        paths,
        truncated,
        diagnostics: Vec::new(),
    })
}

#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::service::trace::find_start_node_id;

/// CLI wrapper — prints the taint audit receipt to stdout as JSON.
#[cfg(feature = "cli")]
#[forge(
    name = "taint",
    version = "0.4.0",
    description = "Cross-language taint audit. Rules mode (default): match graph nodes against the builtin source/sink library (C/Python/JS/PHP/Solidity) and trace source→sink paths. Manual mode: pass --source and --sink symbol names. Params: source/sink (empty = rules mode); language — filter (e.g. c|python|javascript|php|solidity, empty = all); max_pairs (default 200); depth (default 8); project.",
    cli = true
)]
async fn taint(
    source: String,
    sink: String,
    language: String,
    max_pairs: String,
    depth: String,
    project: String,
) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let output = run_taint(
        &kit,
        &source,
        &sink,
        &language,
        parse_count("max_pairs", &max_pairs).map_err(|e| to_api_error(e, "taint_error"))?,
        parse_count("depth", &depth).map_err(|e| to_api_error(e, "taint_error"))?,
        &project,
    )
    .map_err(|e| to_api_error(e, "taint_error"))?;
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

#[cfg(feature = "cli")]
fn parse_count(name: &str, raw: &str) -> Result<usize, CodeNexusError> {
    raw.trim()
        .parse::<usize>()
        .map_err(|_| CodeNexusError::InvalidInput(format!("invalid --{name}: {raw}")))
}

#[cfg(test)]
#[cfg(any(feature = "cli", feature = "mcp"))]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageModule};
    use tempfile::TempDir;

    fn fresh_kit() -> (TempDir, AsyncKit<AsyncReady>) {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("taint_db");
        let config = KitBootstrapConfig::new(db);
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .unwrap();
        (dir, kit)
    }

    fn seed_taint_flow(kit: &AsyncKit<AsyncReady>) {
        let storage = kit.require::<StorageModule>().unwrap();
        // C-language flow: getenv → parse → system. Language lives on File
        // nodes (function tables have no language column), so seed File rows
        // matching each function's filePath.
        for (id, path) in [
            ("tf_a", "/src/a.c"),
            ("tf_b", "/src/b.c"),
            ("tf_c", "/src/c.c"),
        ] {
            storage
                .execute(&format!(
                    "CREATE (:File {{id: '{id}', project: 'demo', name: '{path}', filePath: '{path}', language: 'c', hash: '', lineCount: 0}});"
                ))
                .unwrap();
        }
        storage
            .execute("CREATE (:Function {id: 't_src', project: 'demo', name: 'getenv', qualifiedName: 'demo.getenv', filePath: '/src/a.c', startLine: 1, endLine: 2, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});")
            .unwrap();
        storage
            .execute("CREATE (:Function {id: 't_mid', project: 'demo', name: 'parse_cmd', qualifiedName: 'demo.parse_cmd', filePath: '/src/b.c', startLine: 3, endLine: 4, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});")
            .unwrap();
        storage
            .execute("CREATE (:Function {id: 't_sink', project: 'demo', name: 'system', qualifiedName: 'demo.system', filePath: '/src/c.c', startLine: 5, endLine: 6, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});")
            .unwrap();
        storage
            .execute("CREATE (:CodeRelation {id: 'te1', source: 't_src', target: 't_mid', type: 'DATAFLOWS', confidence: 0.9, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});")
            .unwrap();
        storage
            .execute("CREATE (:CodeRelation {id: 'te2', source: 't_mid', target: 't_sink', type: 'DATAFLOWS', confidence: 0.9, confidenceTier: 'High', reason: '', startLine: 3, project: 'demo'});")
            .unwrap();
    }

    #[test]
    fn rules_mode_finds_seeded_c_flow() {
        let (_dir, kit) = fresh_kit();
        seed_taint_flow(&kit);
        let out = run_taint(&kit, "", "", "c", 200, 8, "").expect("audit runs");
        assert_eq!(out.mode, "rules");
        assert_eq!(out.rules_version.as_deref(), Some("1"));
        assert!(out.sources_found >= 1, "getenv should be a source");
        assert!(out.sinks_found >= 1, "system should be a sink");
        assert!(
            out.paths
                .iter()
                .any(|p| p.source == "getenv" && p.sink == "system"),
            "source→sink path expected: {:?}",
            out.paths
        );
    }

    #[test]
    fn max_pairs_caps_and_flags_truncation() {
        let (_dir, kit) = fresh_kit();
        seed_taint_flow(&kit);
        // Cap of exactly the available pairs → nothing cut, not truncated.
        let out = run_taint(&kit, "", "", "c", 1, 8, "").expect("audit runs");
        assert_eq!(out.pairs_scanned, 1);
        assert!(!out.truncated);
        // Cap of zero cuts before the first pair → truncated flag set.
        let out = run_taint(&kit, "", "", "c", 0, 8, "").expect("audit runs");
        assert_eq!(out.pairs_scanned, 0);
        assert!(out.truncated, "cap must flag truncation");
    }

    #[test]
    fn manual_mode_unknown_sink_is_not_found() {
        let (_dir, kit) = fresh_kit();
        seed_taint_flow(&kit);
        let err = run_taint(&kit, "getenv", "nonexistent_sink_fn", "", 200, 8, "")
            .expect_err("unknown sink must fail");
        assert!(
            matches!(err, CodeNexusError::NotFound(_)),
            "expected NotFound: {err:?}"
        );
        assert!(err.to_string().contains("sink symbol not found"));
    }

    #[test]
    fn manual_mode_traces_between_named_symbols() {
        let (_dir, kit) = fresh_kit();
        seed_taint_flow(&kit);
        let out = run_taint(&kit, "getenv", "system", "", 200, 8, "").expect("manual mode runs");
        assert_eq!(out.mode, "manual");
        assert_eq!(out.pairs_scanned, 1);
        assert!(!out.paths.is_empty(), "path expected through parse_cmd");
        assert_eq!(out.paths[0].nodes.len(), 3);
    }

    #[test]
    fn manual_mode_requires_both_endpoints() {
        let (_dir, kit) = fresh_kit();
        seed_taint_flow(&kit);
        let err = run_taint(&kit, "getenv", "", "", 200, 8, "").expect_err("half-specified");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
    }

    #[test]
    fn language_filter_limits_candidates() {
        let (_dir, kit) = fresh_kit();
        seed_taint_flow(&kit);
        // Python has no seeded nodes → nothing found but no error.
        let out = run_taint(&kit, "", "", "python", 200, 8, "").expect("python audit");
        assert_eq!(out.sources_found, 0);
        assert_eq!(out.paths, Vec::<TaintFinding>::new());
    }

    #[test]
    fn parse_count_rejects_garbage() {
        assert!(parse_count("max_pairs", "200").is_ok());
        let err = parse_count("max_pairs", "abc").expect_err("garbage");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
    }
}
