// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Extractor trait (Adapter pattern) and extraction result types.
//!
//! Defines the unified [`Extractor`] trait that each language-specific
//! extractor implements to adapt tree-sitter's syntax tree into CodeNexus
//! nodes and edges (ADR-003, ADR-011).
//!
//! The pure data types ([`ImportInfo`], [`CallInfo`], [`AssignInfo`],
//! [`ExternInfo`], [`ReadInfo`], [`WriteInfo`], [`ExtractResult`]) live in
//! [`crate::ir`] and are re-exported here for backward compatibility.

use std::path::Path;

use crate::model::Language;

use super::error::{ParseError, Result};

// Backward-compatibility re-exports: these types now live in `crate::ir`.
// `pub use` also brings them into the current scope for use in `extract_file`
// and the `Extractor` trait signature.
pub use crate::ir::{
    AssignInfo, CallInfo, ExternInfo, ExtractResult, ImportInfo, ReadInfo, WriteInfo,
};

// ---------------------------------------------------------------------------
// Extractor trait (Adapter pattern, ADR-003).
// ---------------------------------------------------------------------------

/// Adapter trait for language-specific tree-sitter extraction.
///
/// Each language implements this trait to adapt tree-sitter's syntax tree
/// into CodeNexus's unified node/edge model (Adapter pattern, ADR-003).
/// The trait is `Send + Sync` to support parallel parsing with rayon
/// (ADR-010).
///
/// Concrete extractors for C, Rust, Fortran, Python, and TypeScript are
/// implemented in Task 6.
pub trait Extractor: Send + Sync {
    /// Returns the language this extractor handles.
    fn language(&self) -> Language;

    /// Extracts symbols from source code.
    ///
    /// # Arguments
    ///
    /// * `source` - The source code text.
    /// * `file_path` - The path of the source file (used for node `file_path`).
    /// * `project` - The project name (used for node `project` field, DDD §2.3).
    ///
    /// # Returns
    ///
    /// An [`ExtractResult`] containing extracted nodes, edges, and
    /// intermediate records.
    fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult>;
}

// ---------------------------------------------------------------------------
// extract_file: convenience function.
// ---------------------------------------------------------------------------

/// Reads a source file and extracts symbols.
///
/// This is a convenience function that reads the file from disk and dispatches
/// to [`extract_from_source`] (which dispatches to the language-specific
/// [`Extractor`] via [`get_extractor`](super::get_extractor)).
///
/// # Errors
///
/// Returns [`ParseError::Io`] if the file cannot be read, or a
/// [`ParseError::ParseFailed`] if the tree-sitter parser returns no tree.
pub fn extract_file(path: &Path, language: Language, project: &str) -> Result<ExtractResult> {
    let source = std::fs::read_to_string(path).map_err(|source| ParseError::Io {
        file_path: path.display().to_string(),
        source,
    })?;
    extract_from_source(&path.display().to_string(), &source, language, project)
}

/// Extracts symbols from an in-memory source string.
///
/// Used by the RAM-first indexing path (H15): source bytes are LZ4-decompressed
/// into a `String` and passed here directly, bypassing the disk read in
/// [`extract_file`]. `file_path_str` is used only for node `file_path` fields
/// and error messages — no filesystem access occurs.
///
/// # Errors
///
/// Returns a [`ParseError::ParseFailed`] if the tree-sitter parser returns no
/// tree.
pub fn extract_from_source(
    file_path_str: &str,
    source: &str,
    language: Language,
    project: &str,
) -> Result<ExtractResult> {
    let language = maybe_upgrade_h_to_cpp(file_path_str, source, language);
    let extractor = super::get_extractor(language);
    extractor.extract(source, file_path_str, project)
}

/// Upgrades a `.h` file from C to C++ when its content contains C++-only
/// syntax (BUG-C3). `.h` is ambiguous between C and C++; `from_extension`
/// maps it to C by default. This function inspects the source for C++-only
/// keywords (`namespace`, `template`, `class`, access specifiers) and
/// upgrades to Cpp when found, so C++ class/struct definitions in headers
/// are not lost.
#[cfg(all(feature = "lang-c", feature = "lang-cpp"))]
fn maybe_upgrade_h_to_cpp(file_path_str: &str, source: &str, language: Language) -> Language {
    if language == Language::C {
        if let Some(ext) = std::path::Path::new(file_path_str)
            .extension()
            .and_then(|e| e.to_str())
        {
            if ext.eq_ignore_ascii_case("h") && detect_cpp_header(source) {
                return Language::Cpp;
            }
        }
    }
    language
}

#[cfg(not(all(feature = "lang-c", feature = "lang-cpp")))]
fn maybe_upgrade_h_to_cpp(_file_path_str: &str, _source: &str, language: Language) -> Language {
    language
}

/// Returns true if the source contains C++-only syntax markers.
///
/// `namespace`, `template`, and `class` are not C keywords, so their
/// presence (as a word followed by whitespace) strongly indicates C++.
/// Access specifiers (`public:` etc.) are also C++-only.
#[cfg(all(feature = "lang-c", feature = "lang-cpp"))]
fn detect_cpp_header(source: &str) -> bool {
    source.contains("namespace ")
        || source.contains("template ")
        || source.contains("class ")
        || source.contains("public:")
        || source.contains("private:")
        || source.contains("protected:")
}

#[cfg(all(
    test,
    feature = "lang-c",
    feature = "lang-cpp",
    feature = "lang-python",
    feature = "lang-rust"
))]
mod tests {
    use super::*;
    use crate::model::{Edge, EdgeType, Node, NodeLabel};

    // --- ImportInfo tests ---

    #[test]
    fn import_info_can_be_constructed() {
        let info = ImportInfo {
            source_file: "stdio.h".to_string(),
            imported_names: vec!["printf".to_string(), "scanf".to_string()],
            line: 1,
        };
        assert_eq!(info.source_file, "stdio.h");
        assert_eq!(info.imported_names.len(), 2);
        assert_eq!(info.line, 1);
    }

    #[test]
    fn import_info_with_empty_names() {
        let info = ImportInfo {
            source_file: "std::io".to_string(),
            imported_names: vec![],
            line: 5,
        };
        assert!(info.imported_names.is_empty());
    }

    #[test]
    fn import_info_clone_and_eq() {
        let info = ImportInfo {
            source_file: "os".to_string(),
            imported_names: vec!["path".to_string()],
            line: 3,
        };
        let cloned = info.clone();
        assert_eq!(info, cloned);
    }

    // --- CallInfo tests ---

    #[test]
    fn call_info_can_be_constructed() {
        let info = CallInfo {
            caller_qn: Some("main".to_string()),
            callee_name: "printf".to_string(),
            line: 5,
            args: vec!["\"hello\"".to_string()],
        };
        assert_eq!(info.caller_qn.as_deref(), Some("main"));
        assert_eq!(info.callee_name, "printf");
        assert_eq!(info.line, 5);
        assert_eq!(info.args.len(), 1);
    }

    #[test]
    fn call_info_with_no_caller() {
        let info = CallInfo {
            caller_qn: None,
            callee_name: "malloc".to_string(),
            line: 3,
            args: vec![],
        };
        assert!(info.caller_qn.is_none());
        assert!(info.args.is_empty());
    }

    #[test]
    fn call_info_clone_and_eq() {
        let info = CallInfo {
            caller_qn: Some("foo".to_string()),
            callee_name: "bar".to_string(),
            line: 10,
            args: vec!["x".to_string()],
        };
        let cloned = info.clone();
        assert_eq!(info, cloned);
    }

    // --- AssignInfo tests ---

    #[test]
    fn assign_info_can_be_constructed() {
        let info = AssignInfo {
            target_name: "x".to_string(),
            source_name: "foo".to_string(),
            line: 10,
            is_return_assign: true,
        };
        assert_eq!(info.target_name, "x");
        assert_eq!(info.source_name, "foo");
        assert_eq!(info.line, 10);
        assert!(info.is_return_assign);
    }

    #[test]
    fn assign_info_not_return_assign() {
        let info = AssignInfo {
            target_name: "y".to_string(),
            source_name: "z".to_string(),
            line: 20,
            is_return_assign: false,
        };
        assert!(!info.is_return_assign);
    }

    #[test]
    fn assign_info_clone_and_eq() {
        let info = AssignInfo {
            target_name: "a".to_string(),
            source_name: "b".to_string(),
            line: 1,
            is_return_assign: false,
        };
        assert_eq!(info, info.clone());
    }

    // --- ExternInfo tests ---

    #[test]
    fn extern_info_can_be_constructed() {
        let info = ExternInfo {
            language: Language::C,
            names: vec!["printf".to_string()],
            line: 1,
            signature: Some("int printf(const char*, ...)".to_string()),
        };
        assert_eq!(info.language, Language::C);
        assert_eq!(info.names.len(), 1);
        assert_eq!(info.line, 1);
        assert!(info.signature.is_some());
    }

    #[test]
    fn extern_info_without_signature() {
        let info = ExternInfo {
            language: Language::C,
            names: vec![],
            line: 5,
            signature: None,
        };
        assert!(info.names.is_empty());
        assert!(info.signature.is_none());
    }

    #[test]
    fn extern_info_clone_and_eq() {
        let info = ExternInfo {
            language: Language::Rust,
            names: vec!["foo".to_string()],
            line: 2,
            signature: Some("fn foo()".to_string()),
        };
        assert_eq!(info, info.clone());
    }

    // --- ReadInfo tests ---

    #[test]
    fn read_info_can_be_constructed() {
        let info = ReadInfo {
            reader_qn: Some("main".to_string()),
            var_name: "x".to_string(),
            line: 5,
        };
        assert_eq!(info.reader_qn.as_deref(), Some("main"));
        assert_eq!(info.var_name, "x");
        assert_eq!(info.line, 5);
    }

    #[test]
    fn read_info_with_no_reader() {
        let info = ReadInfo {
            reader_qn: None,
            var_name: "y".to_string(),
            line: 3,
        };
        assert!(info.reader_qn.is_none());
    }

    #[test]
    fn read_info_clone_and_eq() {
        let info = ReadInfo {
            reader_qn: Some("foo".to_string()),
            var_name: "bar".to_string(),
            line: 1,
        };
        assert_eq!(info, info.clone());
    }

    // --- WriteInfo tests ---

    #[test]
    fn write_info_can_be_constructed() {
        let info = WriteInfo {
            writer_qn: Some("main".to_string()),
            var_name: "y".to_string(),
            line: 7,
        };
        assert_eq!(info.writer_qn.as_deref(), Some("main"));
        assert_eq!(info.var_name, "y");
        assert_eq!(info.line, 7);
    }

    #[test]
    fn write_info_with_no_writer() {
        let info = WriteInfo {
            writer_qn: None,
            var_name: "z".to_string(),
            line: 4,
        };
        assert!(info.writer_qn.is_none());
    }

    #[test]
    fn write_info_clone_and_eq() {
        let info = WriteInfo {
            writer_qn: Some("foo".to_string()),
            var_name: "baz".to_string(),
            line: 2,
        };
        assert_eq!(info, info.clone());
    }

    // --- ExtractResult tests ---

    #[test]
    fn extract_result_new_is_empty() {
        let result = ExtractResult::new("test.rs", Language::Rust);
        assert_eq!(result.file_path, "test.rs");
        assert_eq!(result.language, Language::Rust);
        assert!(result.nodes.is_empty());
        assert!(result.edges.is_empty());
        assert!(result.imports.is_empty());
        assert!(result.calls.is_empty());
        assert!(result.assignments.is_empty());
        assert!(result.externs.is_empty());
        assert!(result.reads.is_empty());
        assert!(result.writes.is_empty());
        assert!(result.is_empty());
    }

    #[test]
    fn extract_result_new_accepts_string_and_str() {
        let result = ExtractResult::new(String::from("test.rs"), Language::Rust);
        assert_eq!(result.file_path, "test.rs");
    }

    #[test]
    fn extract_result_can_be_constructed_with_all_fields() {
        let result = ExtractResult {
            file_path: "test.rs".to_string(),
            language: Language::Rust,
            nodes: vec![Node::builder(NodeLabel::Function, "main", "main")
                .language(Language::Rust)
                .build()],
            edges: vec![Edge::new("a", "b", EdgeType::Calls, "proj")],
            imports: vec![ImportInfo {
                source_file: "std::io".to_string(),
                imported_names: vec!["println".to_string()],
                line: 1,
            }],
            calls: vec![CallInfo {
                caller_qn: Some("main".to_string()),
                callee_name: "println".to_string(),
                line: 3,
                args: vec![],
            }],
            assignments: vec![AssignInfo {
                target_name: "x".to_string(),
                source_name: "5".to_string(),
                line: 5,
                is_return_assign: false,
            }],
            externs: vec![ExternInfo {
                language: Language::C,
                names: vec!["printf".to_string()],
                line: 7,
                signature: None,
            }],
            reads: vec![ReadInfo {
                reader_qn: Some("main".to_string()),
                var_name: "x".to_string(),
                line: 8,
            }],
            writes: vec![WriteInfo {
                writer_qn: Some("main".to_string()),
                var_name: "y".to_string(),
                line: 9,
            }],
            seen_qns: std::collections::HashSet::new(),
        };
        assert_eq!(result.file_path, "test.rs");
        assert_eq!(result.language, Language::Rust);
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.edges.len(), 1);
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.calls.len(), 1);
        assert_eq!(result.assignments.len(), 1);
        assert_eq!(result.externs.len(), 1);
        assert_eq!(result.reads.len(), 1);
        assert_eq!(result.writes.len(), 1);
        assert!(!result.is_empty());
    }

    #[test]
    fn extract_result_is_empty_when_only_some_fields_populated() {
        let mut result = ExtractResult::new("test.rs", Language::Rust);
        result.push_node(Node::builder(NodeLabel::Function, "foo", "foo").build());
        assert!(!result.is_empty(), "result with nodes should not be empty");

        let mut result2 = ExtractResult::new("test.rs", Language::Rust);
        result2.imports.push(ImportInfo {
            source_file: "os".to_string(),
            imported_names: vec![],
            line: 1,
        });
        assert!(
            !result2.is_empty(),
            "result with imports should not be empty"
        );
    }

    #[test]
    fn extract_result_clone_preserves_data() {
        let mut result = ExtractResult::new("test.rs", Language::Rust);
        result.push_node(Node::builder(NodeLabel::Function, "foo", "foo").build());
        let cloned = result.clone();
        assert_eq!(cloned.file_path, result.file_path);
        assert_eq!(cloned.language, result.language);
        assert_eq!(cloned.nodes.len(), result.nodes.len());
    }

    // --- extract_file tests ---

    #[test]
    fn extract_file_nonexistent_returns_error() {
        let result = extract_file(
            std::path::Path::new("/nonexistent/path/does_not_exist.rs"),
            Language::Rust,
            "proj",
        );
        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::Io { file_path, source } => {
                assert!(
                    file_path.contains("does_not_exist.rs"),
                    "error should contain file path: {file_path}"
                );
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("expected Io error, got {other:?}"),
        }
    }

    #[test]
    fn extract_file_reads_existing_file() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(temp.path(), "fn main() {}").unwrap();
        let result = extract_file(temp.path(), Language::Rust, "proj");
        assert!(
            result.is_ok(),
            "extract_file should succeed for existing file"
        );
        let result = result.unwrap();
        assert_eq!(result.language, Language::Rust);
        // Task 6: extract_file now dispatches to the language-specific extractor.
        assert!(
            !result.is_empty(),
            "result should contain extracted symbols after Task 6"
        );
        assert!(
            result.nodes.iter().any(|n| n.name == "main"),
            "should extract the main function: {:?}",
            result.nodes.iter().map(|n| &n.name).collect::<Vec<_>>()
        );
    }

    // --- Extractor trait tests ---

    /// A dummy extractor used to verify the trait can be implemented and used.
    struct DummyExtractor {
        lang: Language,
    }

    impl Extractor for DummyExtractor {
        fn language(&self) -> Language {
            self.lang
        }

        fn extract(&self, source: &str, file_path: &str, project: &str) -> Result<ExtractResult> {
            let mut result = ExtractResult::new(file_path, self.lang);
            if !source.is_empty() {
                result.push_node(
                    Node::builder(NodeLabel::Function, "dummy", format!("{project}.dummy"))
                        .language(self.lang)
                        .file_path(file_path)
                        .project(project)
                        .build(),
                );
            }
            Ok(result)
        }
    }

    #[test]
    fn extractor_trait_can_be_implemented() {
        let ext = DummyExtractor {
            lang: Language::Rust,
        };
        assert_eq!(ext.language(), Language::Rust);
    }

    #[test]
    fn extractor_extract_returns_result_with_nodes() {
        let ext = DummyExtractor {
            lang: Language::Rust,
        };
        let result = ext.extract("fn main() {}", "test.rs", "proj").unwrap();
        assert_eq!(result.language, Language::Rust);
        assert_eq!(result.file_path, "test.rs");
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].name, "dummy");
        assert_eq!(result.nodes[0].project, "proj");
        assert_eq!(result.nodes[0].file_path.as_deref(), Some("test.rs"));
        assert_eq!(result.nodes[0].language, Some(Language::Rust));
    }

    #[test]
    fn extractor_extract_empty_source_returns_empty_result() {
        let ext = DummyExtractor {
            lang: Language::Python,
        };
        let result = ext.extract("", "empty.py", "proj").unwrap();
        assert_eq!(result.language, Language::Python);
        assert!(result.nodes.is_empty());
        assert!(result.is_empty());
    }

    #[test]
    fn extractor_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<DummyExtractor>();
        // Also verify the trait object is Send + Sync.
        fn assert_dyn_extractor_send_sync(x: &dyn Extractor) {
            let _ = &x;
        }
        let ext = DummyExtractor {
            lang: Language::Rust,
        };
        assert_dyn_extractor_send_sync(&ext);
    }

    #[test]
    fn extractor_can_be_used_as_trait_object() {
        let extractors: Vec<Box<dyn Extractor>> = vec![
            Box::new(DummyExtractor {
                lang: Language::Rust,
            }),
            Box::new(DummyExtractor {
                lang: Language::Python,
            }),
        ];
        assert_eq!(extractors.len(), 2);
        assert_eq!(extractors[0].language(), Language::Rust);
        assert_eq!(extractors[1].language(), Language::Python);

        let result = extractors[0].extract("x", "a.rs", "p").unwrap();
        assert_eq!(result.nodes.len(), 1);
    }

    // --- BUG-C3: .h file C/C++ content detection ---

    #[cfg(all(feature = "lang-c", feature = "lang-cpp"))]
    #[test]
    fn extract_from_source_upgrades_h_with_class_to_cpp() {
        // BUG-C3: .h files with C++ syntax (class/namespace/template) should
        // be parsed as C++, not C. Without this, C++ class/struct definitions
        // in .h files are lost (fmt class=77 vs 414, struct=268 vs 576).
        let src = "class Foo { public: int x; };\n";
        let result = extract_from_source("test.h", src, Language::C, "proj").unwrap();
        assert_eq!(
            result.language,
            Language::Cpp,
            ".h with class keyword should be upgraded to C++"
        );
        let classes: Vec<_> = result
            .nodes
            .iter()
            .filter(|n| n.label == NodeLabel::Class)
            .collect();
        assert_eq!(
            classes.len(),
            1,
            "C++ class in .h should be extracted: {:?}",
            result.nodes
        );
    }

    #[cfg(all(feature = "lang-c", feature = "lang-cpp"))]
    #[test]
    fn extract_from_source_upgrades_h_with_namespace_to_cpp() {
        let src = "namespace ns { void foo() {} }\n";
        let result = extract_from_source("test.h", src, Language::C, "proj").unwrap();
        assert_eq!(result.language, Language::Cpp);
    }

    #[cfg(all(feature = "lang-c", feature = "lang-cpp"))]
    #[test]
    fn extract_from_source_keeps_h_without_cpp_keywords_as_c() {
        // A pure C header should remain parsed as C.
        let src = "#ifndef FOO_H\n#define FOO_H\nint add(int a, int b);\n#endif\n";
        let result = extract_from_source("test.h", src, Language::C, "proj").unwrap();
        assert_eq!(result.language, Language::C, "pure C header should stay C");
    }

    #[cfg(all(feature = "lang-c", feature = "lang-cpp"))]
    #[test]
    fn extract_from_source_does_not_upgrade_c_file() {
        // .c files must never be upgraded to C++ even if they contain C++ keywords.
        let src = "int add(int a, int b) { return a + b; }\n";
        let result = extract_from_source("test.c", src, Language::C, "proj").unwrap();
        assert_eq!(result.language, Language::C, ".c file must stay C");
    }
}
