//! Symbol resolution and data-flow analysis.
//!
//! Generates fully-qualified names (ADD §7.1), maintains scope chains and
//! symbol tables, and resolves call/data-flow/FFI edges (ADR-011).
//!
//! # Modules
//!
//! - [`error`]: [`ResolveError`] and [`Result`](error::Result) alias.
//! - [`fqn`]: [`FqnGenerator`] for ADD §7.1 FQN generation.
//! - [`scope`]: [`Scope`] and [`ScopeChain`] for nested scope resolution.
//! - [`symbol_table`]: [`SymbolEntry`], [`FileSymbolTable`],
//!   [`ProjectSymbolTable`] for symbol indexing.

pub mod error;
pub mod fqn;
pub mod scope;
pub mod symbol_table;

pub use error::{ResolveError, Result};
pub use fqn::FqnGenerator;
pub use scope::{Scope, ScopeChain};
pub use symbol_table::{FileSymbolTable, ProjectSymbolTable, SymbolEntry};

use crate::parse::ExtractResult;

/// Builds a project-level symbol table from extraction results.
///
/// For each [`ExtractResult`], generates FQNs for all definition nodes and
/// registers them in both the file-level and project-level tables.
///
/// # Arguments
///
/// * `results` - The extraction results from the parse phase.
/// * `project` - The project name used as the FQN prefix.
///
/// # Returns
///
/// A [`ProjectSymbolTable`] containing all symbols indexed by name (global)
/// and by file path (file-scoped).
#[must_use]
pub fn build_symbol_table(results: &[ExtractResult], project: &str) -> ProjectSymbolTable {
    let mut table = ProjectSymbolTable::new();
    for result in results {
        let mut file_table = FileSymbolTable::new();
        for node in &result.nodes {
            let entity_name = &node.name;
            let language = node.language.unwrap_or(result.language);
            let fqn = FqnGenerator::generate(project, &result.file_path, entity_name, language);
            let entry = SymbolEntry::new(
                entity_name.clone(),
                fqn,
                node.label,
                result.file_path.clone(),
                project,
            )
            .with_language(language)
            .with_exported(node.is_exported)
            .with_signature_opt(node.signature.clone());
            file_table.add(entry);
        }
        if !file_table.is_empty() {
            table.add_file_table(&result.file_path, file_table);
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Language, Node, NodeLabel};
    use crate::parse::{CallInfo, ImportInfo};

    fn make_node(name: &str, label: NodeLabel, language: Language) -> Node {
        Node::builder(label, name, "placeholder")
            .language(language)
            .build()
    }

    fn make_result(file_path: &str, language: Language, nodes: Vec<Node>) -> ExtractResult {
        let mut result = ExtractResult::new(file_path, language);
        result.nodes = nodes;
        result
    }

    // --- build_symbol_table ---

    #[test]
    fn build_from_empty_results() {
        let table = build_symbol_table(&[], "proj");
        assert_eq!(table.file_count(), 0);
        assert_eq!(table.symbol_count(), 0);
    }

    #[test]
    fn build_from_single_result_single_node() {
        let node = make_node("parse", NodeLabel::Function, Language::Rust);
        let result = make_result("src/main.rs", Language::Rust, vec![node]);
        let table = build_symbol_table(&[result], "myproject");
        assert_eq!(table.file_count(), 1);
        assert_eq!(table.symbol_count(), 1);
        let entries = table.lookup("parse");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].qn, "myproject.src.main.parse");
        assert_eq!(entries[0].file_path, "src/main.rs");
        assert_eq!(entries[0].language, Some(Language::Rust));
    }

    #[test]
    fn build_from_multiple_results() {
        let r1 = make_result(
            "src/main.rs",
            Language::Rust,
            vec![make_node("main", NodeLabel::Function, Language::Rust)],
        );
        let r2 = make_result(
            "src/utils.rs",
            Language::Rust,
            vec![make_node("helper", NodeLabel::Function, Language::Rust)],
        );
        let table = build_symbol_table(&[r1, r2], "proj");
        assert_eq!(table.file_count(), 2);
        assert_eq!(table.symbol_count(), 2);
        assert_eq!(table.lookup("main")[0].qn, "proj.src.main.main");
        assert_eq!(table.lookup("helper")[0].qn, "proj.src.utils.helper");
    }

    #[test]
    fn build_generates_correct_fqns() {
        let r1 = make_result(
            "src/deep/file.rs",
            Language::Rust,
            vec![make_node("foo", NodeLabel::Function, Language::Rust)],
        );
        let table = build_symbol_table(&[r1], "proj");
        let entry = table.lookup_exact("foo").unwrap();
        assert_eq!(entry.qn, "proj.src.deep.file.foo");
    }

    #[test]
    fn build_python_init_py_fqn() {
        let r = make_result(
            "src/pkg/__init__.py",
            Language::Python,
            vec![make_node("MyClass", NodeLabel::Class, Language::Python)],
        );
        let table = build_symbol_table(&[r], "proj");
        let entry = table.lookup_exact("MyClass").unwrap();
        assert_eq!(entry.qn, "proj.src.pkg.MyClass");
    }

    #[test]
    fn build_c_header_fqn() {
        let r = make_result(
            "include/header.h",
            Language::C,
            vec![make_node("MY_DEFINE", NodeLabel::Const, Language::C)],
        );
        let table = build_symbol_table(&[r], "proj");
        let entry = table.lookup_exact("MY_DEFINE").unwrap();
        assert_eq!(entry.qn, "proj.include.header.MY_DEFINE");
    }

    #[test]
    fn build_fortran_module_fqn_via_generate_for_module() {
        // Direct test of generate_for_module since build_symbol_table uses
        // generate() (top-level entities). Module-nested entities would be
        // handled by a higher-level resolver.
        let fqn = FqnGenerator::generate_for_module("proj", "src/mod.f90", "mymod", "my_func");
        assert_eq!(fqn, "proj.src.mod.mymod.my_func");
    }

    #[test]
    fn build_symbols_added_to_correct_file_tables() {
        let r1 = make_result(
            "a.rs",
            Language::Rust,
            vec![make_node("foo", NodeLabel::Function, Language::Rust)],
        );
        let r2 = make_result(
            "b.rs",
            Language::Rust,
            vec![make_node("bar", NodeLabel::Function, Language::Rust)],
        );
        let table = build_symbol_table(&[r1, r2], "proj");
        assert_eq!(table.lookup_in_file("a.rs", "foo").len(), 1);
        assert!(table.lookup_in_file("a.rs", "bar").is_empty());
        assert_eq!(table.lookup_in_file("b.rs", "bar").len(), 1);
        assert!(table.lookup_in_file("b.rs", "foo").is_empty());
    }

    #[test]
    fn build_preserves_exported_flag() {
        let node = Node::builder(NodeLabel::Function, "foo", "qn")
            .language(Language::Rust)
            .is_exported(true)
            .build();
        let result = make_result("src/main.rs", Language::Rust, vec![node]);
        let table = build_symbol_table(&[result], "proj");
        let entry = table.lookup_exact("foo").unwrap();
        assert!(entry.is_exported);
    }

    #[test]
    fn build_preserves_signature() {
        let node = Node::builder(NodeLabel::Function, "foo", "qn")
            .language(Language::Rust)
            .signature("fn foo(x: i32) -> i32")
            .build();
        let result = make_result("src/main.rs", Language::Rust, vec![node]);
        let table = build_symbol_table(&[result], "proj");
        let entry = table.lookup_exact("foo").unwrap();
        assert_eq!(entry.signature.as_deref(), Some("fn foo(x: i32) -> i32"));
    }

    #[test]
    fn build_preserves_label() {
        let node = make_node("MyClass", NodeLabel::Class, Language::Rust);
        let result = make_result("src/main.rs", Language::Rust, vec![node]);
        let table = build_symbol_table(&[result], "proj");
        let entry = table.lookup_exact("MyClass").unwrap();
        assert_eq!(entry.label, NodeLabel::Class);
    }

    #[test]
    fn build_skips_empty_results() {
        let r1 = make_result("a.rs", Language::Rust, vec![]);
        let r2 = make_result(
            "b.rs",
            Language::Rust,
            vec![make_node("foo", NodeLabel::Function, Language::Rust)],
        );
        let table = build_symbol_table(&[r1, r2], "proj");
        assert_eq!(table.file_count(), 1);
        assert_eq!(table.symbol_count(), 1);
    }

    #[test]
    fn build_multiple_nodes_same_file() {
        let r = make_result(
            "src/main.rs",
            Language::Rust,
            vec![
                make_node("foo", NodeLabel::Function, Language::Rust),
                make_node("bar", NodeLabel::Function, Language::Rust),
                make_node("MyClass", NodeLabel::Class, Language::Rust),
            ],
        );
        let table = build_symbol_table(&[r], "proj");
        assert_eq!(table.symbol_count(), 3);
        assert_eq!(table.file_count(), 1);
        assert_eq!(table.lookup("foo").len(), 1);
        assert_eq!(table.lookup("bar").len(), 1);
        assert_eq!(table.lookup("MyClass").len(), 1);
    }

    #[test]
    fn build_cross_file_lookup() {
        let r1 = make_result(
            "a.rs",
            Language::Rust,
            vec![make_node("foo", NodeLabel::Function, Language::Rust)],
        );
        let r2 = make_result(
            "b.rs",
            Language::Rust,
            vec![make_node("foo", NodeLabel::Function, Language::Rust)],
        );
        let table = build_symbol_table(&[r1, r2], "proj");
        let results = table.lookup("foo");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn build_uses_result_language_when_node_has_none() {
        let mut node = make_node("foo", NodeLabel::Function, Language::Rust);
        node.language = None;
        let result = make_result("src/main.rs", Language::Rust, vec![node]);
        let table = build_symbol_table(&[result], "proj");
        let entry = table.lookup_exact("foo").unwrap();
        assert_eq!(entry.language, Some(Language::Rust));
    }

    #[test]
    fn build_ignores_imports_and_calls() {
        let mut result = ExtractResult::new("src/main.rs", Language::Rust);
        result.nodes.push(make_node("foo", NodeLabel::Function, Language::Rust));
        result.imports.push(ImportInfo {
            source_file: "std::io".to_string(),
            imported_names: vec!["println".to_string()],
            line: 1,
        });
        result.calls.push(CallInfo {
            caller_qn: Some("foo".to_string()),
            callee_name: "println".to_string(),
            line: 3,
            args: vec![],
        });
        let table = build_symbol_table(&[result], "proj");
        // Only the node should be in the symbol table.
        assert_eq!(table.symbol_count(), 1);
        assert_eq!(table.lookup("foo").len(), 1);
        assert!(table.lookup("println").is_empty());
    }

    #[test]
    fn build_typescript_fqn() {
        let r = make_result(
            "src/components/Button.tsx",
            Language::TypeScript,
            vec![make_node("Button", NodeLabel::Class, Language::TypeScript)],
        );
        let table = build_symbol_table(&[r], "proj");
        let entry = table.lookup_exact("Button").unwrap();
        assert_eq!(entry.qn, "proj.src.components.Button.Button");
    }

    #[test]
    fn build_normalizes_dot_slash_path() {
        let r = make_result(
            "./src/main.rs",
            Language::Rust,
            vec![make_node("foo", NodeLabel::Function, Language::Rust)],
        );
        let table = build_symbol_table(&[r], "proj");
        let entry = table.lookup_exact("foo").unwrap();
        assert_eq!(entry.qn, "proj.src.main.foo");
    }

    #[test]
    fn build_exported_lookup_works() {
        let node = Node::builder(NodeLabel::Function, "foo", "qn")
            .language(Language::Rust)
            .is_exported(true)
            .build();
        let result = make_result("src/main.rs", Language::Rust, vec![node]);
        let table = build_symbol_table(&[result], "proj");
        let exported = table.lookup_exported("foo");
        assert_eq!(exported.len(), 1);
    }

    #[test]
    fn build_all_symbols_returns_all() {
        let r = make_result(
            "src/main.rs",
            Language::Rust,
            vec![
                make_node("foo", NodeLabel::Function, Language::Rust),
                make_node("bar", NodeLabel::Function, Language::Rust),
            ],
        );
        let table = build_symbol_table(&[r], "proj");
        let all = table.all_symbols();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn build_preserves_project_field() {
        let node = make_node("foo", NodeLabel::Function, Language::Rust);
        let result = make_result("src/main.rs", Language::Rust, vec![node]);
        let table = build_symbol_table(&[result], "myproject");
        let entry = table.lookup_exact("foo").unwrap();
        assert_eq!(entry.project, "myproject");
    }
}
