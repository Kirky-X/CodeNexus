// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

use std::collections::HashSet;

use crate::ir::ExtractResult;
use crate::model::{Edge, EdgeType, Language, Node as ModelNode, NodeLabel};
use crate::storage::schema::escape_cypher_string;

/// Confidence for File → ExternalPackage DEPENDS_ON edges.
pub const CONFIDENCE_DEPENDS_ON: f32 = 0.9;

/// Derives the package root from an import path for `language`.
#[must_use]
pub fn package_root(import_path: &str, language: Language) -> String {
    match language {
        // Scoped npm packages: `@scope/name` — keep both segments.
        Language::JavaScript | Language::TypeScript if import_path.starts_with('@') => {
            import_path.split('/').take(2).collect::<Vec<_>>().join("/")
        }
        // Rust `serde_json::de` → first `::` segment.
        Language::Rust => import_path
            .split("::")
            .find(|s| !s.is_empty())
            .unwrap_or(import_path)
            .to_string(),
        // Python `os.path` → first `.` segment.
        Language::Python => import_path
            .split('.')
            .find(|s| !s.is_empty())
            .unwrap_or(import_path)
            .to_string(),
        // Go module paths and everything else: first `/`-segment.
        _ => import_path
            .split('/')
            .find(|s| !s.is_empty())
            .unwrap_or(import_path)
            .to_string(),
    }
}

/// Maps a language to its package ecosystem name.
#[must_use]
pub fn ecosystem_for(language: Language) -> &'static str {
    match language {
        Language::Rust => "crates",
        Language::Python => "pypi",
        Language::JavaScript | Language::TypeScript => "npm",
        Language::Go => "gomodules",
        Language::Java => "maven",
        Language::CSharp => "nuget",
        Language::Ruby => "rubygems",
        Language::Php => "packagist",
        _ => "other",
    }
}

/// Stable node id for an external package within a project.
#[must_use]
fn package_node_id(project: &str, root: &str) -> String {
    format!(
        "extpkg:{}:{}",
        escape_cypher_string(project),
        escape_cypher_string(root)
    )
}

/// Records one external dependency: creates the `ExternalPackage` node once
/// per (project, package root) and deduplicates `DEPENDS_ON` edges per
/// (file, package) pair. Called from the unresolved-import branch of
/// `ImportResolver::resolve_imports`.
pub fn record_external_dependency(
    graph: &mut crate::model::Graph,
    source_file_id: &str,
    import_path: &str,
    language: Language,
    line: u32,
    project: &str,
    seen_pairs: &mut HashSet<(String, String)>,
) {
    if import_path.trim().is_empty() {
        return;
    }
    let root = package_root(import_path, language);
    if root.is_empty() {
        return;
    }
    let node_id = package_node_id(project, &root);
    if graph.get_node(&node_id).is_none() {
        // Node::builder generates its own id — override with the stable
        // package id so dedup works across files.
        let mut package =
            ModelNode::builder(NodeLabel::ExternalPackage, root.clone(), node_id.clone())
                .language(language)
                .project(project)
                .properties(serde_json::json!({ "ecosystem": ecosystem_for(language) }))
                .build();
        package.id = node_id.clone();
        graph.add_node(package);
    }

    let pair_key = (source_file_id.to_string(), node_id.clone());
    if seen_pairs.insert(pair_key) {
        // CodeRelation has no import-path column; the reason field carries
        // the originating import statement.
        let edge = Edge::builder(
            source_file_id,
            node_id.clone(),
            EdgeType::DependsOn,
            project,
        )
        .confidence(CONFIDENCE_DEPENDS_ON)
        .start_line(line)
        .reason(format!("import: {import_path}"))
        .build();
        graph.add_edge(edge);
    }
}

/// Convenience wrapper for tests and the `supply` command: processes a batch
/// of extraction results against an already-resolved graph.
pub fn collect_external_dependencies(
    graph: &mut crate::model::Graph,
    results: &[ExtractResult],
    project: &str,
    unresolved: &[(usize, &crate::ir::ImportInfo)],
    source_file_ids: &[String],
) {
    let mut seen = HashSet::new();
    for (result_idx, import) in unresolved {
        let Some(file_id) = source_file_ids.get(*result_idx) else {
            continue;
        };
        record_external_dependency(
            graph,
            file_id,
            &import.source_file,
            results[*result_idx].language,
            import.line,
            project,
            &mut seen,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_root_per_language() {
        assert_eq!(package_root("serde_json::de", Language::Rust), "serde_json");
        assert_eq!(package_root("os.path", Language::Python), "os");
        assert_eq!(
            package_root("@angular/core", Language::TypeScript),
            "@angular/core"
        );
        assert_eq!(
            package_root("@angular/core", Language::JavaScript),
            "@angular/core"
        );
        assert_eq!(package_root("github.com/x/y/z", Language::Go), "github.com");
        assert_eq!(
            package_root("unknown_pkg", Language::Haskell),
            "unknown_pkg"
        );
    }

    #[test]
    fn ecosystem_mapping() {
        assert_eq!(ecosystem_for(Language::Rust), "crates");
        assert_eq!(ecosystem_for(Language::Python), "pypi");
        assert_eq!(ecosystem_for(Language::TypeScript), "npm");
        assert_eq!(ecosystem_for(Language::Go), "gomodules");
        assert_eq!(ecosystem_for(Language::C), "other");
    }

    #[test]
    fn depends_on_edges_dedup_per_file_and_node_dedups_per_package() {
        let mut graph = crate::model::Graph::new();
        graph.add_node(
            crate::model::Node::builder(NodeLabel::File, "a.rs", "file_a")
                .project("demo")
                .build(),
        );
        graph.add_node(
            crate::model::Node::builder(NodeLabel::File, "b.rs", "file_b")
                .project("demo")
                .build(),
        );
        let mut seen = HashSet::new();
        for _ in 0..3 {
            record_external_dependency(
                &mut graph,
                "file_a",
                "serde_json::de",
                Language::Rust,
                1,
                "demo",
                &mut seen,
            );
        }
        record_external_dependency(
            &mut graph,
            "file_b",
            "serde_json::ser",
            Language::Rust,
            2,
            "demo",
            &mut seen,
        );

        let ext_nodes: Vec<_> = graph
            .nodes
            .values()
            .filter(|n| n.label == NodeLabel::ExternalPackage)
            .collect();
        assert_eq!(ext_nodes.len(), 1, "one package node for serde_json");
        let deps: Vec<_> = graph
            .edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::DependsOn)
            .collect();
        assert_eq!(deps.len(), 2, "one edge per (file, package) pair");
    }
}
