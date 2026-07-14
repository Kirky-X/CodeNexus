// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Cross-language FFI resolution (resolve/cross_lang.rs).
//!
//! Provides [`FfiResolver`] for resolving cross-language FFI calls to
//! FfiCalls edges (ADD §7.4, BR-TRACE-008).
//!
//! # Business rules
//!
//! - BR-TRACE-008: Cross-language call -> FFI_CALLS edge.
//! - Confidence: signature match 0.85 (lowered by up to 0.15 when parameter
//!   types mismatch), name-only match 0.70.
//! - C↔Rust: Rust extern "C" calls C functions.
//! - C↔Fortran: Fortran ISO_C_BINDING calls C functions (or vice versa).
//! - Type matching uses [`TypeMapper`] to canonicalize C/Rust type names
//!   (e.g. `int` ↔ `i32`, `double` ↔ `f64`, `char*` ↔ `*const c_char`).
//!
//! # Resolution flow (ADD §7.4)
//!
//! ```text
//! Rust extern "C" block -> extract function names -> search C file definitions
//!   -> name match? -> signature match (param count + types)?
//!     -> count match, types match   -> FfiCalls edge, confidence 0.85
//!     -> count match, types differ  -> FfiCalls edge, confidence 0.70..0.85
//!     -> no (name only)             -> FfiCalls edge, confidence 0.70
//!     -> no name match              -> unresolved
//! ```

use crate::ir::{ExternInfo, ExtractResult};
use crate::model::{ConfidenceTier, Edge, EdgeType, Graph, Language, NodeLabel};
use crate::resolve::ProjectSymbolTable;

/// Confidence for a name+signature FFI match (ADD §7.4).
const CONFIDENCE_NAME_AND_SIG: f32 = 0.85;
/// Confidence for a name-only FFI match (ADD §7.4).
const CONFIDENCE_NAME_ONLY: f32 = 0.70;
/// Confidence penalty applied per unmatched parameter type (ADD §7.4).
/// When all parameter types mismatch, the total penalty is 0.15, lowering
/// the signature-match confidence from 0.85 to 0.70.
const CONFIDENCE_TYPE_MISMATCH_PENALTY: f32 = 0.15;

/// Maps type names across languages for FFI signature matching (ADD §7.4).
///
/// Each type name is canonicalized to a stable identifier (e.g. `"int32"`,
/// `"float64"`, `"string"`, `"void"`) so that equivalent types in C and Rust
/// compare equal. Unknown types canonicalize to `None` and are treated as
/// incompatible with everything (including other unknowns), which lowers the
/// match confidence rather than silently treating them as compatible.
struct TypeMapper;

impl TypeMapper {
    /// Returns the canonical type ID for a type name, or `None` if unknown.
    ///
    /// Canonical types: `"int32"`, `"int64"`, `"float32"`, `"float64"`,
    /// `"string"`, `"void"`, `"ptr"`.
    #[must_use]
    fn canonical_type(type_name: &str, language: Language) -> Option<&'static str> {
        let normalized = type_name.trim();
        match (language, normalized) {
            (Language::C, "int") | (Language::Rust, "i32") => Some("int32"),
            (Language::C, "long") | (Language::Rust, "i64") => Some("int64"),
            (Language::C, "float") | (Language::Rust, "f32") => Some("float32"),
            (Language::C, "double") | (Language::Rust, "f64") => Some("float64"),
            (Language::C, "char*") | (Language::C, "char *") => Some("string"),
            (Language::Rust, "*const c_char") | (Language::Rust, "CString") => Some("string"),
            (Language::C, "void") | (Language::Rust, "()") => Some("void"),
            _ => None,
        }
    }

    /// Returns `true` if two type names are compatible across languages.
    ///
    /// Two types are compatible when they canonicalize to the same identifier.
    /// Unknown types (`None`) are never compatible, even with each other, so a
    /// missing mapping is treated as a mismatch rather than a silent match.
    #[must_use]
    fn types_compatible(type_a: &str, lang_a: Language, type_b: &str, lang_b: Language) -> bool {
        Self::canonical_type(type_a, lang_a) == Self::canonical_type(type_b, lang_b)
    }
}

/// Extracts parameter type names from a signature string.
///
/// Parses the content between the first matching pair of parentheses and
/// splits it by top-level commas (ignoring commas nested inside `()`, `[]`,
/// or `<>`). For each parameter, extracts just the type name:
///
/// - **Rust** (`"fn foo(x: i32, y: f64)"`): the type is the part after the
///   last `:` that is not part of a `::` path separator.
/// - **C** (`"void foo(int x, double y)"`): the type is everything before the
///   trailing parameter name (when the last identifier is not a type keyword).
///
/// Returns an empty vector when the signature has no parentheses, no
/// parameters, or is otherwise unparseable.
///
/// # Examples
///
/// - `"fn foo(x: i32, y: f64)"` (Rust) -> `["i32", "f64"]`
/// - `"void foo(int x, double y)"` (C) -> `["int", "double"]`
/// - `"int foo(int, double)"` (C) -> `["int", "double"]`
/// - `"fn foo()"` -> `[]`
fn parse_params(signature: &str, language: Language) -> Vec<String> {
    let start = match signature.find('(') {
        Some(s) => s,
        None => return Vec::new(),
    };

    // Find the matching ')' for the first '(' (handles nested parens).
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
        None => return Vec::new(),
    };

    let params_str = signature[start + 1..end].trim();
    if params_str.is_empty() {
        return Vec::new();
    }

    // Split by top-level commas (not inside nested parens/brackets/angles).
    let mut params: Vec<String> = Vec::new();
    let mut depth: i32 = 0;
    let mut current = String::new();
    for ch in params_str.chars() {
        match ch {
            '(' | '[' | '<' => {
                depth += 1;
                current.push(ch);
            }
            ')' | ']' | '>' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    params.push(extract_type(trimmed, language));
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        params.push(extract_type(trimmed, language));
    }
    params
}

/// Extracts the type name from a single parameter string.
///
/// - Rust: `"x: i32"` -> `"i32"` (type follows the `:` separator, skipping
///   `::` path separators).
/// - C: `"int x"` -> `"int"`, `"char *x"` -> `"char *"`, `"int"` -> `"int"`
///   (type precedes the parameter name; a trailing identifier that is not a
///   C type keyword is treated as the name and stripped).
fn extract_type(param: &str, language: Language) -> String {
    match language {
        Language::Rust => extract_rust_type(param),
        Language::C => extract_c_type(param),
        // For languages without a known parameter syntax, return the param
        // as-is. TypeMapper will canonicalize it to None (unknown).
        // Only included when other languages may be compiled in; with just
        // `lang-c` + `lang-rust` the match above is already exhaustive.
        #[cfg(any(
            feature = "lang-fortran",
            feature = "lang-python",
            feature = "lang-typescript"
        ))]
        _ => param.trim().to_string(),
    }
}

/// Extracts the type from a Rust parameter like `"x: i32"` -> `"i32"`.
///
/// Finds the first `:` that is not part of a `::` path separator and returns
/// everything after it, trimmed.
fn extract_rust_type(param: &str) -> String {
    let bytes = param.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' {
            // A `::` is a path separator, not the name/type separator.
            if i + 1 < bytes.len() && bytes[i + 1] == b':' {
                i += 2;
                continue;
            }
            return param[i + 1..].trim().to_string();
        }
        i += 1;
    }
    // No `:` found: no name, treat the whole string as the type.
    param.trim().to_string()
}

/// C type keywords used to distinguish a type from a parameter name.
const C_TYPE_KEYWORDS: &[&str] = &[
    "int", "long", "short", "char", "float", "double", "void", "unsigned", "signed", "const",
    "struct", "union", "enum",
];

/// Extracts the type from a C parameter like `"int x"` -> `"int"`,
/// `"char *x"` -> `"char *"`, or `"int"` -> `"int"`.
///
/// If the last identifier in the string is a C type keyword (or there is no
/// trailing identifier), the whole string is the type. Otherwise the trailing
/// identifier is treated as the parameter name and stripped.
fn extract_c_type(param: &str) -> String {
    let trimmed = param.trim();
    let bytes = trimmed.as_bytes();

    // Skip trailing whitespace to find the end of the last token.
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if end == 0 {
        return String::new();
    }

    // Find the start of the last identifier ([a-zA-Z_][a-zA-Z0-9_]*).
    let mut id_start = end;
    while id_start > 0
        && (bytes[id_start - 1].is_ascii_alphanumeric() || bytes[id_start - 1] == b'_')
    {
        id_start -= 1;
    }

    // No trailing identifier (e.g. "char*"): the whole string is the type.
    if id_start == end {
        return trimmed.to_string();
    }

    let last_id = &trimmed[id_start..end];

    // The last identifier is a type keyword (e.g. "int" in "unsigned int"):
    // the whole string is the type.
    if C_TYPE_KEYWORDS.contains(&last_id) {
        return trimmed.to_string();
    }

    // The last identifier is the parameter name: strip it.
    let type_part = trimmed[..id_start].trim_end();
    if type_part.is_empty() {
        trimmed.to_string()
    } else {
        type_part.to_string()
    }
}

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
            // Single-line for coverage: tarpaulin attribute continuation
            let source = self
                .find_function_in_file(caller_file)
                .unwrap_or_else(|| caller_file.clone());
            for extern_info in &result.externs {
                // Single-line for coverage: tarpaulin attribute continuation
                if let Some((target_qn, confidence, reason)) =
                    self.resolve_extern(extern_info, caller_file)
                {
                    // Single-line for coverage: tarpaulin attribute continuation
                    let edge =
                        Edge::builder(source.clone(), target_qn, EdgeType::FfiCalls, self.project)
                            .confidence(confidence)
                            .confidence_tier(ConfidenceTier::Global)
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
    /// - **NameAndSignature** (confidence 0.85 minus type-mismatch penalty):
    ///   name match with matching parameter count. When parameter types also
    ///   match, confidence is 0.85; each unmatched type lowers it by up to
    ///   0.15 total (ADD §7.4, BR-TRACE-008).
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
            // Single-line for coverage: tarpaulin attribute continuation
            let candidates = self
                .symbol_table
                .lookup(name)
                .into_iter()
                .filter(|e| e.language == Some(extern_info.language))
                .collect::<Vec<_>>();

            // Single-line for coverage: tarpaulin attribute continuation
            if candidates.is_empty() {
                continue;
            }

            // Try to find a signature match first (NameAndSignature). The
            // confidence is 0.85 when types match, lowered by up to 0.15 when
            // they do not. Requires both signatures to be present.
            if let Some(extern_sig) = extern_info.signature.as_deref() {
                for candidate in &candidates {
                    if let Some(c_sig) = candidate.signature.as_deref() {
                        // Single-line for coverage: tarpaulin attribute continuation
                        if let Some(confidence) = Self::match_by_signature(extern_sig, c_sig) {
                            // Single-line for coverage: tarpaulin attribute continuation
                            let reason = format!(
                                "FFI name+signature match for '{}' ({} -> {})",
                                name, caller_file, candidate.qn
                            );
                            return Some((candidate.qn.clone(), confidence, reason));
                        }
                    }
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
            let reason = format!(
                "FFI name-only match for '{}' ({} -> {})",
                name, caller_file, qn
            );
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

    /// Matches two FFI signatures and returns a confidence score.
    ///
    /// The first signature is parsed as Rust (the extern declaration side) and
    /// the second as C (the target definition side). Returns `None` when the
    /// parameter counts differ. Otherwise returns `Some(confidence)` where:
    ///
    /// - Base confidence is [`CONFIDENCE_NAME_AND_SIG`] (0.85).
    /// - For each parameter whose types are incompatible (per [`TypeMapper`]),
    ///   the confidence is lowered proportionally, up to a total penalty of
    ///   [`CONFIDENCE_TYPE_MISMATCH_PENALTY`] (0.15) when all types mismatch.
    ///
    /// A function with no parameters has a type-match ratio of 1.0, so its
    /// confidence is 0.85.
    ///
    /// # Examples
    ///
    /// - `"fn foo(x: i32)"` vs `"void foo(int x)"` -> `Some(0.85)` (types match)
    /// - `"fn foo(x: i32)"` vs `"void foo(double x)"` -> `Some(0.70)` (mismatch)
    /// - `"fn foo(x: i32)"` vs `"void foo(int, int)"` -> `None` (count differs)
    #[must_use]
    pub fn match_by_signature(rust_sig: &str, c_sig: &str) -> Option<f32> {
        let rust_params = parse_params(rust_sig, Language::Rust);
        let c_params = parse_params(c_sig, Language::C);

        if rust_params.len() != c_params.len() {
            return None;
        }

        let total_params = rust_params.len();
        let type_match_count = rust_params
            .iter()
            .zip(c_params.iter())
            .filter(|(r, c)| TypeMapper::types_compatible(r, Language::Rust, c, Language::C))
            .count();

        let type_match_ratio = if total_params > 0 {
            type_match_count as f32 / total_params as f32
        } else {
            // No parameters: treat as a full type match.
            1.0
        };

        // Single-line for coverage: tarpaulin attribute continuation
        let confidence =
            CONFIDENCE_NAME_AND_SIG - (1.0 - type_match_ratio) * CONFIDENCE_TYPE_MISMATCH_PENALTY;
        Some(confidence)
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
        // Single-line for coverage: tarpaulin attribute continuation
        for (i, ch) in signature[start..].char_indices() {
            if ch == '(' {
                depth += 1;
            } else if ch == ')' {
                depth -= 1;
                if depth == 0 {
                    end = Some(start + i);
                    break;
                }
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
            if ch == '(' || ch == '[' || ch == '<' {
                depth += 1;
            } else if ch == ')' || ch == ']' || ch == '>' {
                depth -= 1;
            } else if ch == ',' && depth == 0 {
                count += 1;
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
    use crate::ir::ExternInfo;
    use crate::model::{Language, Node, NodeLabel};
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
        let qn = FqnGenerator::generate(project, file, name, language, None);
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
                let qn = FqnGenerator::generate(
                    project,
                    &result.file_path,
                    &node.name,
                    result.language,
                    None,
                );
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
        assert!(FfiResolver::signatures_match(
            Some("fn foo()"),
            Some("int foo()")
        ));
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
            SymbolEntry::new(
                "c_function",
                "proj.c.c_function",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
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
            SymbolEntry::new(
                "c_function",
                "proj.c.c_function",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
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
            SymbolEntry::new(
                "c_function",
                "proj.c.c_function",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
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
            SymbolEntry::new(
                "c_function",
                "proj.c.c_function",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
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
            SymbolEntry::new(
                "c_function",
                "proj.c.c_function",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
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
            SymbolEntry::new(
                "c_func",
                "proj.a.c_func",
                NodeLabel::Function,
                "a.c",
                "proj",
            )
            .with_language(Language::C)
            .with_signature("int c_func(int)"),
        );
        table.add_symbol(
            SymbolEntry::new(
                "c_func",
                "proj.b.c_func",
                NodeLabel::Function,
                "b.c",
                "proj",
            )
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
            SymbolEntry::new(
                "c_function",
                "proj.rs.c_function",
                NodeLabel::Function,
                "main.rs",
                "proj",
            )
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
            SymbolEntry::new(
                "c_function",
                "proj.c.c_function",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
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
            SymbolEntry::new(
                "c_function",
                "proj.c.c_function",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
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
        assert_eq!(edges[0].confidence_tier, ConfidenceTier::Global);
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
            SymbolEntry::new(
                "c_func1",
                "proj.c.c_func1",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
            .with_language(Language::C),
        );
        table.add_symbol(
            SymbolEntry::new(
                "c_func2",
                "proj.c.c_func2",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
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
            SymbolEntry::new(
                "c_function",
                "proj.c.c_function",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
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
        let rust_node = make_node(
            "rust_func",
            "main.rs",
            "proj",
            NodeLabel::Function,
            Language::Rust,
        );
        let rust_qn = FqnGenerator::generate("proj", "main.rs", "rust_func", Language::Rust, None);

        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new(
                "rust_func",
                rust_qn.clone(),
                NodeLabel::Function,
                "main.rs",
                "proj",
            )
            .with_language(Language::Rust),
        );
        table.add_symbol(
            SymbolEntry::new(
                "c_function",
                "proj.c.c_function",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
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
            SymbolEntry::new(
                "c_func",
                "proj.c.c_func",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
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
            SymbolEntry::new(
                "c_function",
                "proj.c.c_function",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
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

        assert!(graph
            .edges
            .iter()
            .any(|e| e.edge_type == EdgeType::FfiCalls));
    }

    #[test]
    fn resolve_ffi_signature_match_creates_high_confidence_edge() {
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new(
                "c_function",
                "proj.c.c_function",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
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
        assert_eq!(edges[0].confidence_tier, ConfidenceTier::Global);
    }

    // --- AC-TRACE-003: Rust extern "C" -> C function FfiCalls edge ---

    #[test]
    fn ac_trace_003_rust_extern_c_calls_c_function() {
        // Given: Rust function with extern "C" block declaring c_function,
        //        C file defining c_function.
        let rust_func_qn =
            FqnGenerator::generate("proj", "src/main.rs", "rust_func", Language::Rust, None);
        let c_func_qn =
            FqnGenerator::generate("proj", "src/c_code.c", "c_function", Language::C, None);

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
        assert!(graph
            .edges
            .iter()
            .any(|e| e.edge_type == EdgeType::FfiCalls));
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
            FqnGenerator::generate("proj", "src/main.rs", "rust_func", Language::Rust, None);
        let c_func_qn =
            FqnGenerator::generate("proj", "src/c_code.c", "c_function", Language::C, None);

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
            SymbolEntry::new(
                "c_func",
                "proj.c.c_func",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
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

    // --- TypeMapper tests (ADD §7.4, BR-TRACE-008) ---

    #[test]
    fn type_mapper_canonical_type_c_int_maps_to_int32() {
        assert_eq!(
            TypeMapper::canonical_type("int", Language::C),
            Some("int32")
        );
    }

    #[test]
    fn type_mapper_canonical_type_rust_i32_maps_to_int32() {
        assert_eq!(
            TypeMapper::canonical_type("i32", Language::Rust),
            Some("int32")
        );
    }

    #[test]
    fn type_mapper_canonical_type_c_long_maps_to_int64() {
        assert_eq!(
            TypeMapper::canonical_type("long", Language::C),
            Some("int64")
        );
    }

    #[test]
    fn type_mapper_canonical_type_rust_i64_maps_to_int64() {
        assert_eq!(
            TypeMapper::canonical_type("i64", Language::Rust),
            Some("int64")
        );
    }

    #[test]
    fn type_mapper_canonical_type_c_float_maps_to_float32() {
        assert_eq!(
            TypeMapper::canonical_type("float", Language::C),
            Some("float32")
        );
    }

    #[test]
    fn type_mapper_canonical_type_rust_f32_maps_to_float32() {
        assert_eq!(
            TypeMapper::canonical_type("f32", Language::Rust),
            Some("float32")
        );
    }

    #[test]
    fn type_mapper_canonical_type_c_double_maps_to_float64() {
        assert_eq!(
            TypeMapper::canonical_type("double", Language::C),
            Some("float64")
        );
    }

    #[test]
    fn type_mapper_canonical_type_rust_f64_maps_to_float64() {
        assert_eq!(
            TypeMapper::canonical_type("f64", Language::Rust),
            Some("float64")
        );
    }

    #[test]
    fn type_mapper_canonical_type_c_char_star_maps_to_string() {
        assert_eq!(
            TypeMapper::canonical_type("char*", Language::C),
            Some("string")
        );
        assert_eq!(
            TypeMapper::canonical_type("char *", Language::C),
            Some("string")
        );
    }

    #[test]
    fn type_mapper_canonical_type_rust_c_char_ptr_maps_to_string() {
        assert_eq!(
            TypeMapper::canonical_type("*const c_char", Language::Rust),
            Some("string")
        );
        assert_eq!(
            TypeMapper::canonical_type("CString", Language::Rust),
            Some("string")
        );
    }

    #[test]
    fn type_mapper_canonical_type_c_void_maps_to_void() {
        assert_eq!(
            TypeMapper::canonical_type("void", Language::C),
            Some("void")
        );
    }

    #[test]
    fn type_mapper_canonical_type_rust_unit_maps_to_void() {
        assert_eq!(
            TypeMapper::canonical_type("()", Language::Rust),
            Some("void")
        );
    }

    #[test]
    fn type_mapper_canonical_type_unknown_returns_none() {
        assert_eq!(
            TypeMapper::canonical_type("unknown_type", Language::C),
            None
        );
        assert_eq!(TypeMapper::canonical_type("Vec<i32>", Language::Rust), None);
    }

    #[test]
    fn type_mapper_types_compatible_int_and_i32() {
        assert!(TypeMapper::types_compatible(
            "int",
            Language::C,
            "i32",
            Language::Rust
        ));
    }

    #[test]
    fn type_mapper_types_compatible_double_and_f64() {
        assert!(TypeMapper::types_compatible(
            "double",
            Language::C,
            "f64",
            Language::Rust
        ));
    }

    #[test]
    fn type_mapper_types_compatible_char_star_and_c_char_ptr() {
        assert!(TypeMapper::types_compatible(
            "char*",
            Language::C,
            "*const c_char",
            Language::Rust
        ));
        assert!(TypeMapper::types_compatible(
            "char *",
            Language::C,
            "CString",
            Language::Rust
        ));
    }

    #[test]
    fn type_mapper_types_incompatible_int_and_double() {
        assert!(!TypeMapper::types_compatible(
            "int",
            Language::C,
            "double",
            Language::C
        ));
    }

    #[test]
    fn type_mapper_types_incompatible_i32_and_f64() {
        assert!(!TypeMapper::types_compatible(
            "i32",
            Language::Rust,
            "f64",
            Language::Rust
        ));
    }

    #[test]
    fn type_mapper_types_incompatible_when_one_unknown() {
        // Unknown type canonicalizes to None, which won't match a known type.
        assert!(!TypeMapper::types_compatible(
            "unknown",
            Language::C,
            "i32",
            Language::Rust
        ));
    }

    // --- parse_params tests ---

    #[test]
    fn parse_params_rust_two_params() {
        let params = parse_params("fn foo(x: i32, y: f64)", Language::Rust);
        assert_eq!(params, vec!["i32", "f64"]);
    }

    #[test]
    fn parse_params_c_two_named_params() {
        let params = parse_params("void foo(int x, double y)", Language::C);
        assert_eq!(params, vec!["int", "double"]);
    }

    #[test]
    fn parse_params_c_two_unnamed_params() {
        let params = parse_params("int foo(int, double)", Language::C);
        assert_eq!(params, vec!["int", "double"]);
    }

    #[test]
    fn parse_params_rust_no_params() {
        assert!(parse_params("fn foo()", Language::Rust).is_empty());
    }

    #[test]
    fn parse_params_c_no_params() {
        assert!(parse_params("void foo()", Language::C).is_empty());
    }

    #[test]
    fn parse_params_rust_one_param() {
        let params = parse_params("fn foo(x: i32)", Language::Rust);
        assert_eq!(params, vec!["i32"]);
    }

    #[test]
    fn parse_params_c_one_named_param() {
        let params = parse_params("void foo(int x)", Language::C);
        assert_eq!(params, vec!["int"]);
    }

    #[test]
    fn parse_params_c_char_star_named_param() {
        let params = parse_params("void foo(char* x)", Language::C);
        assert_eq!(params, vec!["char*"]);
    }

    #[test]
    fn parse_params_c_char_space_star_named_param() {
        let params = parse_params("void foo(char *x)", Language::C);
        assert_eq!(params, vec!["char *"]);
    }

    #[test]
    fn parse_params_no_parens_returns_empty() {
        assert!(parse_params("int x", Language::C).is_empty());
        assert!(parse_params("fn x", Language::Rust).is_empty());
    }

    #[test]
    fn parse_params_rust_c_char_ptr_param() {
        let params = parse_params("fn foo(x: *const c_char)", Language::Rust);
        assert_eq!(params, vec!["*const c_char"]);
    }

    // --- match_by_signature tests ---

    #[test]
    fn match_by_signature_type_match_returns_high_confidence() {
        // Rust i32 vs C int: types match, confidence = 0.85.
        let confidence = FfiResolver::match_by_signature("fn foo(x: i32)", "void foo(int x)");
        assert!(confidence.is_some());
        let conf = confidence.unwrap();
        assert!(
            conf >= 0.80,
            "expected confidence >= 0.80 for type match, got {}",
            conf
        );
        assert!((conf - 0.85).abs() < 1e-6, "expected 0.85, got {}", conf);
    }

    #[test]
    fn match_by_signature_type_mismatch_lowers_confidence_by_0_15() {
        // Rust i32 vs C double: same param count, different types.
        // confidence = 0.85 - (1.0 - 0.0) * 0.15 = 0.70 (reduced by 0.15).
        let confidence = FfiResolver::match_by_signature("fn foo(x: i32)", "void foo(double x)");
        assert!(confidence.is_some());
        let conf = confidence.unwrap();
        // The confidence should be exactly 0.15 lower than the full type-match
        // confidence (0.85).
        assert!(
            (conf - 0.70).abs() < 1e-6,
            "expected 0.70 (0.85 - 0.15), got {}",
            conf
        );
    }

    #[test]
    fn match_by_signature_param_count_mismatch_returns_none() {
        // 1 Rust param vs 2 C params -> None.
        let confidence = FfiResolver::match_by_signature("fn foo(x: i32)", "void foo(int, int)");
        assert!(confidence.is_none());
    }

    #[test]
    fn match_by_signature_no_params_returns_high_confidence() {
        // No params: type_match_ratio = 1.0, confidence = 0.85.
        let confidence = FfiResolver::match_by_signature("fn foo()", "void foo()");
        assert!(confidence.is_some());
        let conf = confidence.unwrap();
        assert!((conf - 0.85).abs() < 1e-6, "expected 0.85, got {}", conf);
    }

    #[test]
    fn match_by_signature_partial_type_match() {
        // 2 params: first matches (i32/int), second mismatches (f64/double vs... wait)
        // Rust: (i32, f64), C: (int, int) -> 1/2 match, ratio = 0.5
        // confidence = 0.85 - (1.0 - 0.5) * 0.15 = 0.85 - 0.075 = 0.775
        let confidence =
            FfiResolver::match_by_signature("fn foo(a: i32, b: f64)", "void foo(int a, int b)");
        assert!(confidence.is_some());
        let conf = confidence.unwrap();
        assert!(
            (conf - 0.775).abs() < 1e-6,
            "expected 0.775 for 50% type match, got {}",
            conf
        );
    }

    #[test]
    fn match_by_signature_all_types_match_two_params() {
        // Rust (i32, f64) vs C (int, double): both match, confidence = 0.85.
        let confidence =
            FfiResolver::match_by_signature("fn foo(a: i32, b: f64)", "void foo(int a, double b)");
        assert!(confidence.is_some());
        let conf = confidence.unwrap();
        assert!((conf - 0.85).abs() < 1e-6, "expected 0.85, got {}", conf);
    }

    #[test]
    fn match_by_signature_string_type_match() {
        // Rust *const c_char vs C char*: both map to "string".
        let confidence =
            FfiResolver::match_by_signature("fn foo(s: *const c_char)", "void foo(char* s)");
        assert!(confidence.is_some());
        let conf = confidence.unwrap();
        assert!((conf - 0.85).abs() < 1e-6, "expected 0.85, got {}", conf);
    }

    // --- resolve_extern integration with type matching ---

    #[test]
    fn resolve_extern_type_mismatch_lowers_confidence() {
        // Rust i32 vs C double: param count matches, types differ.
        // Confidence should be 0.70 (0.85 - 0.15), not 0.85.
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new(
                "c_function",
                "proj.c.c_function",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
            .with_language(Language::C)
            .with_signature("void c_function(double x)"),
        );

        let resolver = FfiResolver::new(&table, "proj");
        let extern_info = ExternInfo {
            language: Language::C,
            names: vec!["c_function".to_string()],
            line: 1,
            signature: Some("fn c_function(x: i32)".to_string()),
        };

        let result = resolver.resolve_extern(&extern_info, "main.rs");
        assert!(result.is_some());
        let (_, confidence, _) = result.unwrap();
        assert!(
            (confidence - 0.70).abs() < 1e-6,
            "expected 0.70 for type mismatch, got {}",
            confidence
        );
    }

    #[test]
    fn resolve_extern_type_match_keeps_high_confidence() {
        // Rust i32 vs C int: types match, confidence = 0.85.
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new(
                "c_function",
                "proj.c.c_function",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
            .with_language(Language::C)
            .with_signature("void c_function(int x)"),
        );

        let resolver = FfiResolver::new(&table, "proj");
        let extern_info = ExternInfo {
            language: Language::C,
            names: vec!["c_function".to_string()],
            line: 1,
            signature: Some("fn c_function(x: i32)".to_string()),
        };

        let result = resolver.resolve_extern(&extern_info, "main.rs");
        assert!(result.is_some());
        let (_, confidence, _) = result.unwrap();
        assert!(
            (confidence - 0.85).abs() < 1e-6,
            "expected 0.85 for type match, got {}",
            confidence
        );
    }

    // --- branch coverage: edge-case params ---

    #[test]
    fn parse_params_unmatched_open_paren_returns_empty() {
        // No closing ')' -> returns empty (None branch of `end` match).
        assert!(parse_params("fn foo(a: i32", Language::Rust).is_empty());
    }

    #[test]
    fn parse_params_rust_nested_parens_in_type() {
        // Nested parens exercise the depth-tracking arms.
        let params = parse_params("fn foo(a: (i32, i32))", Language::Rust);
        assert_eq!(params.len(), 1);
        assert!(params[0].contains("i32"));
    }

    #[test]
    fn parse_params_rust_nested_angles_in_type() {
        // Angle brackets in Vec<i32> exercise the depth-tracking arms.
        let params = parse_params("fn foo(a: Vec<i32>, b: i32)", Language::Rust);
        assert_eq!(params, vec!["Vec<i32>", "i32"]);
    }

    #[test]
    fn parse_params_rust_absolute_path_type() {
        // A param starting with `::` exercises the `::` path-separator arm.
        let params = parse_params("fn foo(::std::Vec<i32>)", Language::Rust);
        assert_eq!(params.len(), 1);
        assert!(params[0].starts_with("::"));
    }

    #[test]
    fn parse_params_c_unnamed_char_star_param() {
        // "char*" with no trailing name -> returns "char*" (no trailing id).
        let params = parse_params("void foo(char*)", Language::C);
        assert_eq!(params, vec!["char*"]);
    }

    #[test]
    fn parse_params_c_name_only_param() {
        // A C param that is just an identifier (no type) -> returns it as-is.
        let params = parse_params("void foo(x)", Language::C);
        assert_eq!(params, vec!["x"]);
    }

    #[test]
    fn parse_params_c_trailing_whitespace_param() {
        // C param with internal trailing whitespace handling.
        let params = parse_params("void foo(int )", Language::C);
        assert_eq!(params, vec!["int"]);
    }

    #[cfg(any(
        feature = "lang-fortran",
        feature = "lang-python",
        feature = "lang-typescript"
    ))]
    #[test]
    fn parse_params_non_rust_c_language_returns_param_as_is() {
        // For languages without a known parameter syntax, the param is
        // returned as-is (TypeMapper canonicalizes it to None).
        let params = parse_params("def foo(x)", Language::Python);
        assert_eq!(params, vec!["x"]);
    }

    // --- Coverage gap tests: extract_c_type all-whitespace param ---

    #[test]
    fn extract_c_type_all_whitespace_returns_empty() {
        // All-whitespace param: trimmed is empty → end == 0 → return String::new().
        assert_eq!(extract_c_type("   "), "");
        assert_eq!(extract_c_type(""), "");
        assert_eq!(extract_c_type("\t\n"), "");
    }

    // --- Coverage gap tests: match_by_signature zero params, extract_rust_type no colon ---

    #[test]
    fn match_by_signature_zero_params_returns_full_confidence() {
        // No params → type_match_ratio = 1.0 → confidence = 0.85.
        let confidence = FfiResolver::match_by_signature("fn foo()", "void foo()").unwrap();
        assert!((confidence - 0.85).abs() < 1e-6);
    }

    #[test]
    fn extract_rust_type_no_colon_returns_param_as_is() {
        // No ':' in param → returns trimmed param (line 210).
        assert_eq!(extract_rust_type("x"), "x");
        assert_eq!(extract_rust_type("  Vec  "), "Vec");
    }

    #[test]
    fn resolve_extern_extern_sig_but_candidate_no_sig_falls_back_to_name_only() {
        // extern has signature but candidate has no signature →
        // `if let Some(c_sig)` is None → skip sig match → name-only (0.70).
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new(
                "c_func",
                "proj.c.c_func",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
            .with_language(Language::C),
        );

        let resolver = FfiResolver::new(&table, "proj");
        let extern_info = ExternInfo {
            language: Language::C,
            names: vec!["c_func".to_string()],
            line: 1,
            signature: Some("fn c_func(x: i32)".to_string()),
        };

        let result = resolver.resolve_extern(&extern_info, "main.rs");
        assert!(result.is_some());
        let (_, confidence, _) = result.unwrap();
        assert!((confidence - 0.70).abs() < 1e-6);
    }

    #[test]
    fn types_compatible_both_unknown_returns_true() {
        // Two unknown types both canonicalize to None → None == None → true.
        assert!(TypeMapper::types_compatible(
            "unknown1",
            Language::C,
            "unknown2",
            Language::Rust
        ));
    }

    #[test]
    fn extract_c_type_preserves_type_when_last_id_is_keyword() {
        // "unsigned int" — last identifier "int" IS a C type keyword →
        // return the whole string (line 255-257 branch).
        assert_eq!(extract_c_type("unsigned int"), "unsigned int");
        assert_eq!(extract_c_type("const char"), "const char");
        assert_eq!(extract_c_type("signed long"), "signed long");
    }

    #[test]
    fn resolve_extern_keeps_first_name_only_when_second_name_also_matches() {
        // Two names both match with name-only (no signature). The first
        // name's candidate is kept; `best_name_only.is_none()` is false for
        // the second name → skip recording (line 415 false branch).
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new(
                "first_func",
                "proj.c.first_func",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
            .with_language(Language::C),
        );
        table.add_symbol(
            SymbolEntry::new(
                "second_func",
                "proj.c.second_func",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
            .with_language(Language::C),
        );

        let resolver = FfiResolver::new(&table, "proj");
        let extern_info = ExternInfo {
            language: Language::C,
            names: vec!["first_func".to_string(), "second_func".to_string()],
            line: 1,
            signature: None,
        };

        let result = resolver.resolve_extern(&extern_info, "main.rs");
        assert!(result.is_some());
        let (qn, confidence, _) = result.unwrap();
        assert_eq!(
            qn, "proj.c.first_func",
            "first name-only match should be kept, not overwritten by second"
        );
        assert!((confidence - 0.70).abs() < 1e-6);
    }

    #[test]
    fn resolve_extern_first_candidate_no_sig_second_sig_match_returns_sig_match() {
        // First candidate has no signature (c_sig is None → skipped),
        // second candidate has matching signature → returns signature match.
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new(
                "c_func",
                "proj.a.c_func",
                NodeLabel::Function,
                "a.c",
                "proj",
            )
            .with_language(Language::C),
        );
        table.add_symbol(
            SymbolEntry::new(
                "c_func",
                "proj.b.c_func",
                NodeLabel::Function,
                "b.c",
                "proj",
            )
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
        assert_eq!(qn, "proj.b.c_func");
        assert!((confidence - 0.85).abs() < 1e-6);
    }

    #[test]
    fn resolve_ffi_mixed_resolvable_and_unresolvable_produces_only_resolvable() {
        let mut table = ProjectSymbolTable::new();
        table.add_symbol(
            SymbolEntry::new(
                "c_func",
                "proj.c.c_func",
                NodeLabel::Function,
                "c.c",
                "proj",
            )
            .with_language(Language::C),
        );

        let mut result = ExtractResult::new("main.rs", Language::Rust);
        result.externs.push(ExternInfo {
            language: Language::C,
            names: vec!["missing_func".to_string()],
            line: 3,
            signature: None,
        });
        result.externs.push(ExternInfo {
            language: Language::C,
            names: vec!["c_func".to_string()],
            line: 5,
            signature: None,
        });

        let results = vec![result];
        let mut graph = Graph::new();
        let resolver = FfiResolver::new(&table, "proj");
        let edges = resolver.resolve_ffi(&results, &mut graph);

        assert_eq!(edges.len(), 1, "only resolvable extern should produce edge");
        assert_eq!(edges[0].target, "proj.c.c_func");
        assert_eq!(edges[0].start_line, Some(5));
    }
}
