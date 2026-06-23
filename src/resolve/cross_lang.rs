//! Cross-language FFI resolution (resolve/cross_lang.rs).
//!
//! Provides [`FfiResolver`] for resolving cross-language FFI calls to
//! FfiCalls edges (ADD §7.4, BR-TRACE-008).
//!
//! # Business rules
//!
//! - BR-TRACE-008: Cross-language call -> FFI_CALLS edge.
//! - Confidence: signature match 0.85, name-only match 0.70.
//! - C↔Rust: Rust extern "C" calls C functions.
//! - C↔Fortran: Fortran ISO_C_BINDING calls C functions (or vice versa).
//!
//! # Resolution flow (ADD §7.4)
//!
//! ```text
//! Rust extern "C" block -> extract function names -> search C file definitions
//!   -> name match? -> signature match (param count/types)?
//!     -> yes -> FfiCalls edge, confidence 0.85
//!     -> no (name only) -> FfiCalls edge, confidence 0.70
//!     -> no name match -> unresolved
//! ```

use crate::model::{Edge, EdgeType, Graph, NodeLabel};
use crate::parse::{ExternInfo, ExtractResult};
use crate::resolve::ProjectSymbolTable;

/// Confidence for a name+signature FFI match (ADD §7.4).
const CONFIDENCE_NAME_AND_SIG: f32 = 0.85;
/// Confidence for a name-only FFI match (ADD §7.4).
const CONFIDENCE_NAME_ONLY: f32 = 0.70;

/// Match strategy for FFI resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchStrategy {
    /// Name + signature match (confidence 0.85).
    NameAndSignature,
    /// Name-only match (confidence 0.70).
    NameOnly,
    /// No match.
    NoMatch,
}

/// Resolves cross-language FFI calls to FfiCalls edges (ADD §7.4).
///
/// Constructed with a reference to a [`ProjectSymbolTable`] and the project
/// name. Use [`resolve_ffi`] for batch resolution from [`ExtractResult`]s, or
/// [`resolve_extern`] for single-extern resolution.
///
/// [`resolve_ffi`]: FfiResolver::resolve_ffi
/// [`resolve_extern`]: FfiResolver::resolve_extern
pub struct FfiResolver<'a> {
    symbol_table: &'a ProjectSymbolTable,
    project: &'a str,
}

impl<'a> FfiResolver<'a> {
    /// Creates a new `FfiResolver` with the given symbol table and project.
    #[must_use]
    pub fn new(symbol_table: &'a ProjectSymbolTable, project: &'a str) -> Self {
        Self {
            symbol_table,
            project,
        }
    }

    /// Resolves all FFI calls from [`ExtractResult`]s and adds FfiCalls edges
    /// to the graph.
    ///
    /// For each [`ExternInfo`] in each result, resolves the extern declaration
    /// using [`resolve_extern`]. If a match is found, creates an `FfiCalls`
    /// edge from the caller file's function (or the file path itself if no
    /// function is found) to the target definition, and adds it to both the
    /// graph and the returned vector.
    ///
    /// # Arguments
    ///
    /// * `results` - The extraction results containing extern declarations.
    /// * `graph` - The graph to add resolved FfiCalls edges to.
    ///
    /// # Returns
    ///
    /// A vector of all resolved FfiCalls edges (also added to `graph`).
    pub fn resolve_ffi(&self, results: &[ExtractResult], graph: &mut Graph) -> Vec<Edge> {
        let mut edges = Vec::new();
        for result in results {
            let caller_file = &result.file_path;
            // Use a function in the caller file as the edge source if one
            // exists; otherwise fall back to the file path.
            let source = self
                .find_function_in_file(caller_file)
                .unwrap_or_else(|| caller_file.clone());
            for extern_info in &result.externs {
                if let Some((target_qn, confidence, reason)) =
                    self.resolve_extern(extern_info, caller_file)
                {
                    let edge =
                        Edge::builder(source.clone(), target_qn, EdgeType::FfiCalls, self.project)
                            .confidence(confidence)
                            .reason(reason)
                            .start_line(extern_info.line)
                            .build();
                    graph.add_edge(edge.clone());
                    edges.push(edge);
                }
            }
        }
        edges
    }

    /// Resolves a single extern declaration.
    ///
    /// For each name in `extern_info.names`, looks up the name in the symbol
    /// table filtered by the target language (`extern_info.language`). Returns
    /// the best match:
    ///
    /// - **NameAndSignature** (confidence 0.85): name match with matching
    ///   parameter count.
    /// - **NameOnly** (confidence 0.70): name match without signature match
    ///   (or when either signature is unavailable).
    /// - **NoMatch**: no name match in the target language.
    ///
    /// # Arguments
    ///
    /// * `extern_info` - The extern declaration to resolve.
    /// * `caller_file` - The file path of the caller (used in the reason
    ///   string).
    ///
    /// # Returns
    ///
    /// `Some((target_qn, confidence, reason))` if a match is found, `None`
    /// otherwise.
    pub fn resolve_extern(
        &self,
        extern_info: &ExternInfo,
        caller_file: &str,
    ) -> Option<(String, f32, String)> {
        let mut best_name_only: Option<(String, String)> = None;

        for name in &extern_info.names {
            let candidates = self
                .symbol_table
                .lookup(name)
                .into_iter()
                .filter(|e| e.language == Some(extern_info.language))
                .collect::<Vec<_>>();

            if candidates.is_empty() {
                continue;
            }

            // Try to find a signature match first (NameAndSignature, 0.85).
            for candidate in &candidates {
                if Self::signatures_match(
                    extern_info.signature.as_deref(),
                    candidate.signature.as_deref(),
                ) {
                    let reason = format!(
                        "FFI name+signature match for '{}' ({} -> {})",
                        name, caller_file, candidate.qn
                    );
                    return Some((candidate.qn.clone(), CONFIDENCE_NAME_AND_SIG, reason));
                }
            }

            // No signature match; record the first name-only match (0.70).
            if best_name_only.is_none() {
                if let Some(candidate) = candidates.first() {
                    best_name_only = Some((candidate.qn.clone(), name.clone()));
                }
            }
        }

        best_name_only.map(|(qn, name)| {
            let reason = format!("FFI name-only match for '{}' ({} -> {})", name, caller_file, qn);
            (qn, CONFIDENCE_NAME_ONLY, reason)
        })
    }

    /// Checks if two signatures match by comparing parameter counts.
    ///
    /// Returns `true` only when both signatures are present and have the same
    /// parameter count. If either signature is `None`, returns `false` (the
    /// caller should fall back to a name-only match).
    pub fn signatures_match(sig1: Option<&str>, sig2: Option<&str>) -> bool {
        match (sig1, sig2) {
            (Some(s1), Some(s2)) => Self::param_count(s1) == Self::param_count(s2),
            _ => false,
        }
    }

    /// Extracts the parameter count from a signature string.
    ///
    /// Handles both Rust-style signatures (`fn foo(a: i32, b: i32)`) and
    /// C-style signatures (`int foo(int, int)`). Counts top-level commas
    /// (ignoring commas nested inside `()`, `[]`, or `<>`) plus one for the
    /// first parameter.
    ///
    /// # Examples
    ///
    /// - `"fn foo(a: i32, b: i32)"` -> 2
    /// - `"int foo(int, int)"` -> 2
    /// - `"fn foo()"` -> 0
    /// - `""` -> 0
    pub fn param_count(signature: &str) -> usize {
        let start = match signature.find('(') {
            Some(s) => s,
            None => return 0,
        };

        // Find the matching ')' for the first '(' (handles nested parens,
        // e.g. `fn foo(a: (i32, i32))` or `fn foo() -> (i32, i32)`).
        let mut depth: i32 = 0;
        let mut end = None;
        for (i, ch) in signature[start..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(start + i);
                        break;
                    }
                }
                _ => {}
            }
        }

        let end = match end {
            Some(e) => e,
            None => return 0,
        };

        let params = signature[start + 1..end].trim();
        if params.is_empty() {
            return 0;
        }

        // Count top-level commas (not inside nested parens/brackets/angles).
        let mut depth: i32 = 0;
        let mut count = 1;
        for ch in params.chars() {
            match ch {
                '(' | '[' | '<' => depth += 1,
                ')' | ']' | '>' => depth -= 1,
                ',' if depth == 0 => count += 1,
                _ => {}
            }
        }
        count
    }

    /// Finds the first function defined in the given file to use as the edge
    /// source. Returns `None` if no function is found.
    fn find_function_in_file(&self, file: &str) -> Option<String> {
        self.symbol_table
            .all_symbols()
            .into_iter()
            .find(|e| e.file_path == file && e.label == NodeLabel::Function)
            .map(|e| e.qn.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Language, Node, NodeLabel};
    use crate::parse::ExternInfo;
    use crate::resolve::{build_symbol_table, FqnGenerator, ProjectSymbolTable, SymbolEntry};

    // --- helper functions ---

    /// Creates a definition node with the FQN as both `id` and
    /// `qualified_name`.
    fn make_node(
        name: &str,
        file: &str,
        project: &str,
        label: NodeLabel,
        language: Language,
    ) -> Node {
        let qn = FqnGenerator::generate(project, file, name, language);
        Node::builder(label, name, qn)
            .file_path(file)
            .project(project)
            .language(language)
            .build()
    }

    /// Creates an `ExtractResult` with the given nodes.
    fn make_result(file: &str, language: Language, nodes: Vec<Node>) -> ExtractResult {
        let mut result = ExtractResult::new(file, language);
        result.nodes = nodes;
        result
    }

    /// Adds nodes from results to the graph, using each node's FQN as its id.
    fn add_nodes_to_graph(graph: &mut Graph, results: &[ExtractResult], project: &str) {
        for result in results {
            for node in &result.nodes {
                let qn =
                    FqnGenerator::generate(project, &result.file_path, &node.name, result.language);
                let mut graph_node = node.clone();
                graph_node.id = qn.clone();
                graph_node.qualified_name = qn;
                graph.add_node(graph_node);
            }
        }
    }

    // --- param_count tests ---

    #[test]
    fn param_count_rust_two_params() {
        assert_eq!(FfiResolver::param_count("fn foo(a: i32, b: i32)"), 2);
    }

    #[test]
    fn param_count_c_two_params() {
        assert_eq!(FfiResolver::param_count("int foo(int, int)"), 2);
    }

    #[test]
    fn param_count_rust_no_params() {
        assert_eq!(FfiResolver::param_count("fn foo()"), 0);
    }

    #[test]
    fn param_count_empty_string() {
        assert_eq!(FfiResolver::param_count(""), 0);
    }

    #[test]
    fn param_count_rust_one_param() {
        assert_eq!(FfiResolver::param_count("fn foo(x: i32)"), 1);
    }

    #[test]
    fn param_count_c_one_param() {
        assert_eq!(FfiResolver::param_count("int foo(int)"), 1);
    }

    #[test]
    fn param_count_c_three_params() {
        assert_eq!(FfiResolver::param_count("int foo(int, char*, float)"), 3);
    }

    #[test]
    fn param_count_nested_parens() {
        // `fn foo(a: Vec<(i32, i32)>)` -> 1 param (nested comma ignored).
        assert_eq!(FfiResolver::param_count("fn foo(a: Vec<(i32, i32)>)"), 1);
    }

    #[test]
    fn param_count_nested_angles() {
        // `fn foo(a: i32, b: Vec<i32>)` -> 2 params.
        assert_eq!(FfiResolver::param_count("fn foo(a: i32, b: Vec<i32>)"), 2);
    }

    #[test]
    fn param_count_return_tuple() {
        // `fn foo(a: i32) -> (i32, i32)` -> 1 param (return type ignored).
        assert_eq!(FfiResolver::param_count("fn foo(a: i32) -> (i32, i32)"), 1);
    }

    #[test]
    fn param_count_no_parens() {
        assert_eq!(FfiResolver::param_count("int x"), 0);
    }

    #[test]
    fn param_count_unmatched_open_paren() {
        // Malformed: no closing ')' -> 0.
        assert_eq!(FfiResolver::param_count("fn foo(a: i32"), 0);
    }

    // --- signatures_match tests ---

    #[test]
    fn signatures_match_both_none_returns_false() {
        assert!(!FfiResolver::signatures_match(None, None));
    }

    #[test]
    fn signatures_match_one_none_returns_false() {
        assert!(!FfiResolver::signatures_match(Some("fn foo()"), None));
        assert!(!FfiResolver::signatures_match(None, Some("fn foo()")));
    }

    #[test]
    fn signatures_match_same_param_count_returns_true() {
        assert!(FfiResolver::signatures_match(
            Some("fn foo(a: i32, b: i32)"),
            Some("int foo(int, int)"),
        ));
    }

    #[test]
    fn signatures_match_different_param_count_returns_false() {
        assert!(!FfiResolver::signatures_match(
            Some("fn foo(a: i32)"),
            Some("int foo(int, int)"),
        ));
    }

    #[test]
    fn signatures_match_both_zero_params_returns_true() {
        assert!(FfiResolver::signatures_match(Some("fn foo()"), Some("int foo()")));
    }

    // --- MatchStrategy enum tests ---

    #[test]
    fn match_strategy_has_three_variants() {
        assert_eq!(
            MatchStrategy::NameAndSignature,
            MatchStrategy::NameAndSignature
        );
        assert_eq!(MatchStrategy::NameOnly, MatchStrategy::NameOnly);
        assert_eq!(MatchStrategy::NoMatch, MatchStrategy::NoMatch);
        assert_ne!(MatchStrategy::NameAndSignature, MatchStrategy::NameOnly);
        assert_ne!(MatchStrategy::NameAndSignature, MatchStrategy::NoMatch);
        assert_ne!(MatchStrategy::NameOnly, MatchStrategy::NoMatch);
    }

    #[test]
    fn match_strategy_is_copy() {
        let strategy = MatchStrategy::NameAndSignature;
        let copied = strategy;
        assert_eq!(strategy, copied);
    }

    #[test]
    fn match_strategy_debug_contains_variant_name() {
        assert!(format!("{:?}", MatchStrategy::NameAndSignature).contains("NameAndSignature"));
        assert!(format!("{:?}", MatchStrategy::NameOnly).contains("NameOnly"));
        assert!(format!("{:?}", MatchStrategy::NoMatch).contains("NoMatch"));
    }

    // --- resolve_extern tests ---

    #[test]
    fn resolve_extern_name_match_returns_some() {
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new("c_function", "proj.c.c_function", NodeLabel::Function, "c.c", "proj")
                .with_language(Language::C),
        );

        let resolver = FfiResolver::new(&table, "proj");
        let extern_info = ExternInfo {
            language: Language::C,
            names: vec!["c_function".to_string()],
            line: 1,
            signature: None,
        };

        let result = resolver.resolve_extern(&extern_info, "main.rs");
        assert!(result.is_some());
        let (qn, confidence, reason) = result.unwrap();
        assert_eq!(qn, "proj.c.c_function");
        assert!((confidence - 0.70).abs() < 1e-6);
        assert!(reason.contains("name-only"));
    }

    #[test]
    fn resolve_extern_no_match_returns_none() {
        let table = ProjectSymbolTable::new();
        let resolver = FfiResolver::new(&table, "proj");
        let extern_info = ExternInfo {
            language: Language::C,
            names: vec!["missing_function".to_string()],
            line: 1,
            signature: None,
        };
        let result = resolver.resolve_extern(&extern_info, "main.rs");
        assert!(result.is_none());
    }

    #[test]
    fn resolve_extern_signature_match_returns_higher_confidence() {
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new("c_function", "proj.c.c_function", NodeLabel::Function, "c.c", "proj")
                .with_language(Language::C)
                .with_signature("int c_function(int, int)"),
        );

        let resolver = FfiResolver::new(&table, "proj");
        let extern_info = ExternInfo {
            language: Language::C,
            names: vec!["c_function".to_string()],
            line: 1,
            signature: Some("fn c_function(x: i32, y: i32)".to_string()),
        };

        let result = resolver.resolve_extern(&extern_info, "main.rs");
        assert!(result.is_some());
        let (qn, confidence, reason) = result.unwrap();
        assert_eq!(qn, "proj.c.c_function");
        assert!((confidence - 0.85).abs() < 1e-6);
        assert!(reason.contains("name+signature"));
    }

    #[test]
    fn resolve_extern_signature_mismatch_falls_back_to_name_only() {
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new("c_function", "proj.c.c_function", NodeLabel::Function, "c.c", "proj")
                .with_language(Language::C)
                .with_signature("int c_function(int)"),
        );

        let resolver = FfiResolver::new(&table, "proj");
        let extern_info = ExternInfo {
            language: Language::C,
            names: vec!["c_function".to_string()],
            line: 1,
            signature: Some("fn c_function(x: i32, y: i32)".to_string()),
        };

        let result = resolver.resolve_extern(&extern_info, "main.rs");
        assert!(result.is_some());
        let (qn, confidence, _) = result.unwrap();
        assert_eq!(qn, "proj.c.c_function");
        assert!((confidence - 0.70).abs() < 1e-6);
    }

    #[test]
    fn resolve_extern_both_signatures_none_returns_name_only() {
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new("c_function", "proj.c.c_function", NodeLabel::Function, "c.c", "proj")
                .with_language(Language::C),
        );

        let resolver = FfiResolver::new(&table, "proj");
        let extern_info = ExternInfo {
            language: Language::C,
            names: vec!["c_function".to_string()],
            line: 1,
            signature: None,
        };

        let result = resolver.resolve_extern(&extern_info, "main.rs");
        assert!(result.is_some());
        let (qn, confidence, _) = result.unwrap();
        assert_eq!(qn, "proj.c.c_function");
        assert!((confidence - 0.70).abs() < 1e-6);
    }

    #[test]
    fn resolve_extern_multiple_names_returns_best_match() {
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new("c_function", "proj.c.c_function", NodeLabel::Function, "c.c", "proj")
                .with_language(Language::C),
        );

        let resolver = FfiResolver::new(&table, "proj");
        let extern_info = ExternInfo {
            language: Language::C,
            names: vec!["missing_function".to_string(), "c_function".to_string()],
            line: 1,
            signature: None,
        };

        let result = resolver.resolve_extern(&extern_info, "main.rs");
        assert!(result.is_some());
        let (qn, _, _) = result.unwrap();
        assert_eq!(qn, "proj.c.c_function");
    }

    #[test]
    fn resolve_extern_signature_match_takes_precedence_over_name_only() {
        // Two candidates: one with matching signature, one without.
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new("c_func", "proj.a.c_func", NodeLabel::Function, "a.c", "proj")
                .with_language(Language::C)
                .with_signature("int c_func(int)"),
        );
        table.add_symbol(
            SymbolEntry::new("c_func", "proj.b.c_func", NodeLabel::Function, "b.c", "proj")
                .with_language(Language::C)
                .with_signature("int c_func(int, int)"),
        );

        let resolver = FfiResolver::new(&table, "proj");
        let extern_info = ExternInfo {
            language: Language::C,
            names: vec!["c_func".to_string()],
            line: 1,
            signature: Some("fn c_func(x: i32, y: i32)".to_string()),
        };

        let result = resolver.resolve_extern(&extern_info, "main.rs");
        assert!(result.is_some());
        let (qn, confidence, _) = result.unwrap();
        // The signature match (proj.b.c_func, 2 params) should win.
        assert_eq!(qn, "proj.b.c_func");
        assert!((confidence - 0.85).abs() < 1e-6);
    }

    #[test]
    fn resolve_extern_language_mismatch_returns_none() {
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new("c_function", "proj.rs.c_function", NodeLabel::Function, "main.rs", "proj")
                .with_language(Language::Rust),
        );

        let resolver = FfiResolver::new(&table, "proj");
        let extern_info = ExternInfo {
            language: Language::C,
            names: vec!["c_function".to_string()],
            line: 1,
            signature: None,
        };

        let result = resolver.resolve_extern(&extern_info, "main.rs");
        assert!(result.is_none());
    }

    #[test]
    fn resolve_extern_empty_names_returns_none() {
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new("c_function", "proj.c.c_function", NodeLabel::Function, "c.c", "proj")
                .with_language(Language::C),
        );

        let resolver = FfiResolver::new(&table, "proj");
        let extern_info = ExternInfo {
            language: Language::C,
            names: vec![],
            line: 1,
            signature: None,
        };

        let result = resolver.resolve_extern(&extern_info, "main.rs");
        assert!(result.is_none());
    }

    #[test]
    fn resolve_extern_skips_entries_without_language() {
        // SymbolEntry without language should not match (filtered out).
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(SymbolEntry::new(
            "c_function",
            "proj.c.c_function",
            NodeLabel::Function,
            "c.c",
            "proj",
        ));

        let resolver = FfiResolver::new(&table, "proj");
        let extern_info = ExternInfo {
            language: Language::C,
            names: vec!["c_function".to_string()],
            line: 1,
            signature: None,
        };

        let result = resolver.resolve_extern(&extern_info, "main.rs");
        assert!(result.is_none());
    }

    // --- resolve_ffi tests ---

    #[test]
    fn resolve_ffi_creates_edge_for_matching_extern() {
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new("c_function", "proj.c.c_function", NodeLabel::Function, "c.c", "proj")
                .with_language(Language::C),
        );

        let mut result = ExtractResult::new("main.rs", Language::Rust);
        result.externs.push(ExternInfo {
            language: Language::C,
            names: vec!["c_function".to_string()],
            line: 5,
            signature: None,
        });

        let results = vec![result];
        let mut graph = Graph::new();
        let resolver = FfiResolver::new(&table, "proj");
        let edges = resolver.resolve_ffi(&results, &mut graph);

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].edge_type, EdgeType::FfiCalls);
        assert_eq!(edges[0].target, "proj.c.c_function");
        assert!((edges[0].confidence - 0.70).abs() < 1e-6);
        assert_eq!(edges[0].start_line, Some(5));
        assert_eq!(graph.edge_count(), 1);
    }

    #[test]
    fn resolve_ffi_no_externs_returns_empty() {
        let table = ProjectSymbolTable::new();
        let result = ExtractResult::new("main.rs", Language::Rust);
        let results = vec![result];
        let mut graph = Graph::new();
        let resolver = FfiResolver::new(&table, "proj");
        let edges = resolver.resolve_ffi(&results, &mut graph);
        assert!(edges.is_empty());
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn resolve_ffi_empty_results_returns_empty() {
        let table = ProjectSymbolTable::new();
        let mut graph = Graph::new();
        let resolver = FfiResolver::new(&table, "proj");
        let edges = resolver.resolve_ffi(&[], &mut graph);
        assert!(edges.is_empty());
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn resolve_ffi_creates_multiple_edges_for_multiple_externs() {
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new("c_func1", "proj.c.c_func1", NodeLabel::Function, "c.c", "proj")
                .with_language(Language::C),
        );
        table.add_symbol(
            SymbolEntry::new("c_func2", "proj.c.c_func2", NodeLabel::Function, "c.c", "proj")
                .with_language(Language::C),
        );

        let mut result = ExtractResult::new("main.rs", Language::Rust);
        result.externs.push(ExternInfo {
            language: Language::C,
            names: vec!["c_func1".to_string()],
            line: 5,
            signature: None,
        });
        result.externs.push(ExternInfo {
            language: Language::C,
            names: vec!["c_func2".to_string()],
            line: 6,
            signature: None,
        });

        let results = vec![result];
        let mut graph = Graph::new();
        let resolver = FfiResolver::new(&table, "proj");
        let edges = resolver.resolve_ffi(&results, &mut graph);

        assert_eq!(edges.len(), 2);
        assert_eq!(graph.edge_count(), 2);
        assert!(edges.iter().all(|e| e.edge_type == EdgeType::FfiCalls));
    }

    #[test]
    fn resolve_ffi_skips_unresolvable_externs() {
        let table = ProjectSymbolTable::new();
        let mut result = ExtractResult::new("main.rs", Language::Rust);
        result.externs.push(ExternInfo {
            language: Language::C,
            names: vec!["missing_function".to_string()],
            line: 5,
            signature: None,
        });

        let results = vec![result];
        let mut graph = Graph::new();
        let resolver = FfiResolver::new(&table, "proj");
        let edges = resolver.resolve_ffi(&results, &mut graph);

        assert!(edges.is_empty());
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn resolve_ffi_uses_file_as_source_when_no_function_found() {
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new("c_function", "proj.c.c_function", NodeLabel::Function, "c.c", "proj")
                .with_language(Language::C),
        );

        let mut result = ExtractResult::new("main.rs", Language::Rust);
        // No nodes in main.rs, so find_function_in_file returns None.
        result.externs.push(ExternInfo {
            language: Language::C,
            names: vec!["c_function".to_string()],
            line: 5,
            signature: None,
        });

        let results = vec![result];
        let mut graph = Graph::new();
        let resolver = FfiResolver::new(&table, "proj");
        let edges = resolver.resolve_ffi(&results, &mut graph);

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source, "main.rs");
        assert_eq!(edges[0].target, "proj.c.c_function");
    }

    #[test]
    fn resolve_ffi_uses_function_qn_as_source_when_function_found() {
        let rust_node = make_node("rust_func", "main.rs", "proj", NodeLabel::Function, Language::Rust);
        let rust_qn = FqnGenerator::generate("proj", "main.rs", "rust_func", Language::Rust);

        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new("rust_func", rust_qn.clone(), NodeLabel::Function, "main.rs", "proj")
                .with_language(Language::Rust),
        );
        table.add_symbol(
            SymbolEntry::new("c_function", "proj.c.c_function", NodeLabel::Function, "c.c", "proj")
                .with_language(Language::C),
        );

        let mut result = make_result("main.rs", Language::Rust, vec![rust_node]);
        result.externs.push(ExternInfo {
            language: Language::C,
            names: vec!["c_function".to_string()],
            line: 5,
            signature: None,
        });

        let results = vec![result];
        let mut graph = Graph::new();
        let resolver = FfiResolver::new(&table, "proj");
        let edges = resolver.resolve_ffi(&results, &mut graph);

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source, rust_qn);
        assert_eq!(edges[0].target, "proj.c.c_function");
    }

    #[test]
    fn resolve_ffi_handles_multiple_results() {
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new("c_func", "proj.c.c_func", NodeLabel::Function, "c.c", "proj")
                .with_language(Language::C),
        );

        let mut result_a = ExtractResult::new("a.rs", Language::Rust);
        result_a.externs.push(ExternInfo {
            language: Language::C,
            names: vec!["c_func".to_string()],
            line: 5,
            signature: None,
        });
        let mut result_b = ExtractResult::new("b.rs", Language::Rust);
        result_b.externs.push(ExternInfo {
            language: Language::C,
            names: vec!["c_func".to_string()],
            line: 3,
            signature: None,
        });

        let results = vec![result_a, result_b];
        let mut graph = Graph::new();
        let resolver = FfiResolver::new(&table, "proj");
        let edges = resolver.resolve_ffi(&results, &mut graph);

        assert_eq!(edges.len(), 2);
        assert_eq!(graph.edge_count(), 2);
    }

    #[test]
    fn resolve_ffi_adds_edges_to_graph() {
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new("c_function", "proj.c.c_function", NodeLabel::Function, "c.c", "proj")
                .with_language(Language::C),
        );

        let mut result = ExtractResult::new("main.rs", Language::Rust);
        result.externs.push(ExternInfo {
            language: Language::C,
            names: vec!["c_function".to_string()],
            line: 5,
            signature: None,
        });

        let results = vec![result];
        let mut graph = Graph::new();
        let resolver = FfiResolver::new(&table, "proj");
        resolver.resolve_ffi(&results, &mut graph);

        assert!(graph.edges.iter().any(|e| e.edge_type == EdgeType::FfiCalls));
    }

    #[test]
    fn resolve_ffi_signature_match_creates_high_confidence_edge() {
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new("c_function", "proj.c.c_function", NodeLabel::Function, "c.c", "proj")
                .with_language(Language::C)
                .with_signature("int c_function(int, int)"),
        );

        let mut result = ExtractResult::new("main.rs", Language::Rust);
        result.externs.push(ExternInfo {
            language: Language::C,
            names: vec!["c_function".to_string()],
            line: 5,
            signature: Some("fn c_function(a: i32, b: i32)".to_string()),
        });

        let results = vec![result];
        let mut graph = Graph::new();
        let resolver = FfiResolver::new(&table, "proj");
        let edges = resolver.resolve_ffi(&results, &mut graph);

        assert_eq!(edges.len(), 1);
        assert!((edges[0].confidence - 0.85).abs() < 1e-6);
    }

    // --- AC-TRACE-003: Rust extern "C" -> C function FfiCalls edge ---

    #[test]
    fn ac_trace_003_rust_extern_c_calls_c_function() {
        // Given: Rust function with extern "C" block declaring c_function,
        //        C file defining c_function.
        let rust_func_qn =
            FqnGenerator::generate("proj", "src/main.rs", "rust_func", Language::Rust);
        let c_func_qn =
            FqnGenerator::generate("proj", "src/c_code.c", "c_function", Language::C);

        let rust_node = make_node(
            "rust_func",
            "src/main.rs",
            "proj",
            NodeLabel::Function,
            Language::Rust,
        );
        let c_node = make_node(
            "c_function",
            "src/c_code.c",
            "proj",
            NodeLabel::Function,
            Language::C,
        );

        let mut rust_result = make_result("src/main.rs", Language::Rust, vec![rust_node]);
        rust_result.externs.push(ExternInfo {
            language: Language::C,
            names: vec!["c_function".to_string()],
            line: 5,
            signature: None,
        });
        let c_result = make_result("src/c_code.c", Language::C, vec![c_node]);

        let results = vec![rust_result, c_result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();
        add_nodes_to_graph(&mut graph, &results, "proj");

        // When: resolve FFI.
        let resolver = FfiResolver::new(&table, "proj");
        let edges = resolver.resolve_ffi(&results, &mut graph);

        // Then: FfiCalls edge from Rust function to C function in graph.
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].edge_type, EdgeType::FfiCalls);
        assert_eq!(edges[0].source, rust_func_qn);
        assert_eq!(edges[0].target, c_func_qn);

        // Verify: graph has edge with type FfiCalls.
        assert!(graph.edges.iter().any(|e| e.edge_type == EdgeType::FfiCalls));
        let neighbors = graph.neighbors(&rust_func_qn, Some(EdgeType::FfiCalls));
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].id, c_func_qn);
        assert_eq!(neighbors[0].name, "c_function");
    }

    #[test]
    fn ac_trace_003_with_signature_match() {
        // Variant of AC-TRACE-003 where the extern signature matches the C
        // definition signature, yielding confidence 0.85.
        let rust_func_qn =
            FqnGenerator::generate("proj", "src/main.rs", "rust_func", Language::Rust);
        let c_func_qn =
            FqnGenerator::generate("proj", "src/c_code.c", "c_function", Language::C);

        let rust_node = make_node(
            "rust_func",
            "src/main.rs",
            "proj",
            NodeLabel::Function,
            Language::Rust,
        );
        let mut c_node = make_node(
            "c_function",
            "src/c_code.c",
            "proj",
            NodeLabel::Function,
            Language::C,
        );
        c_node.signature = Some("int c_function(int, int)".to_string());

        let mut rust_result = make_result("src/main.rs", Language::Rust, vec![rust_node]);
        rust_result.externs.push(ExternInfo {
            language: Language::C,
            names: vec!["c_function".to_string()],
            line: 5,
            signature: Some("fn c_function(a: i32, b: i32)".to_string()),
        });
        let c_result = make_result("src/c_code.c", Language::C, vec![c_node]);

        let results = vec![rust_result, c_result];
        let table = build_symbol_table(&results, "proj");
        let mut graph = Graph::new();
        add_nodes_to_graph(&mut graph, &results, "proj");

        let resolver = FfiResolver::new(&table, "proj");
        let edges = resolver.resolve_ffi(&results, &mut graph);

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].edge_type, EdgeType::FfiCalls);
        assert_eq!(edges[0].source, rust_func_qn);
        assert_eq!(edges[0].target, c_func_qn);
        assert!((edges[0].confidence - 0.85).abs() < 1e-6);
    }

    // --- Fortran <-> C FFI tests (BR-TRACE-008) ---

    #[test]
    fn resolve_extern_fortran_calls_c_function() {
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new("c_func", "proj.c.c_func", NodeLabel::Function, "c.c", "proj")
                .with_language(Language::C),
        );

        let resolver = FfiResolver::new(&table, "proj");
        // Fortran ISO_C_BINDING declaring a C function.
        let extern_info = ExternInfo {
            language: Language::C,
            names: vec!["c_func".to_string()],
            line: 1,
            signature: None,
        };

        let result = resolver.resolve_extern(&extern_info, "mod.f90");
        assert!(result.is_some());
        let (qn, _, _) = result.unwrap();
        assert_eq!(qn, "proj.c.c_func");
    }

    // --- new constructor test ---

    #[test]
    fn new_creates_resolver() {
        let table = ProjectSymbolTable::new();
        let resolver = FfiResolver::new(&table, "proj");
        // Verify resolve_ffi works on the created resolver.
        let mut graph = Graph::new();
        let edges = resolver.resolve_ffi(&[], &mut graph);
        assert!(edges.is_empty());
    }
}
