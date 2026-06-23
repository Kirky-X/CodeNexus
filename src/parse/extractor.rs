//! Extractor trait (Adapter pattern) and extraction result types.
//!
//! Defines the unified [`Extractor`] trait that each language-specific
//! extractor implements to adapt tree-sitter's syntax tree into CodeNexus
//! nodes and edges (ADR-003, ADR-011).

use std::path::Path;

use crate::model::{Edge, Language, Node};

use super::error::{ParseError, Result};

// ---------------------------------------------------------------------------
// Info structs: intermediate extraction records collected per file.
// ---------------------------------------------------------------------------

/// Information about an import/include statement extracted from source.
///
/// Captured for later resolution of cross-file references (Imports/Includes
/// edges, DDD §7.2).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImportInfo {
    /// The source module or file being imported from
    /// (e.g. `"std::io"`, `"stdio.h"`, `"./utils"`).
    pub source_file: String,
    /// The specific names imported (empty for wildcard/star imports).
    pub imported_names: Vec<String>,
    /// The 1-based line number of the import statement.
    pub line: u32,
}

/// Information about a function or method call extracted from source.
///
/// Captured for later resolution of Calls edges (DDD §7.2).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallInfo {
    /// The qualified name of the calling function/method, if known.
    pub caller_qn: Option<String>,
    /// The name of the called function/method.
    pub callee_name: String,
    /// The 1-based line number of the call expression.
    pub line: u32,
    /// String representations of the call arguments (for data-flow analysis).
    pub args: Vec<String>,
}

/// Information about a variable assignment extracted from source.
///
/// Captured for later resolution of DataFlows/Reads/Writes edges
/// (BR-TRACE-002, BR-TRACE-003).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssignInfo {
    /// The name of the variable being assigned.
    pub target_name: String,
    /// The name of the source expression (variable or function call).
    pub source_name: String,
    /// The 1-based line number of the assignment.
    pub line: u32,
    /// Whether this assignment captures a function return value
    /// (BR-TRACE-002 return assignment).
    pub is_return_assign: bool,
}

/// Information about an extern/FFI declaration extracted from source.
///
/// Captured for later cross-language FFI resolution (ADD §7.4,
/// BR-TRACE-008).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExternInfo {
    /// The foreign language being interfaced with.
    pub language: Language,
    /// The names of the extern symbols declared.
    pub names: Vec<String>,
    /// The 1-based line number of the declaration.
    pub line: u32,
    /// The signature of the extern declaration, if available.
    pub signature: Option<String>,
}

// ---------------------------------------------------------------------------
// ExtractResult: the output of extracting symbols from a single file.
// ---------------------------------------------------------------------------

/// The result of extracting symbols from a single source file.
///
/// Produced by [`Extractor::extract`]. Contains definition nodes, edges,
/// and intermediate records (imports, calls, assignments, externs) used by
/// the downstream resolution phase.
#[derive(Debug, Clone)]
pub struct ExtractResult {
    /// The path of the source file.
    pub file_path: String,
    /// The language of the source file.
    pub language: Language,
    /// Extracted definition nodes (functions, classes, variables, etc.).
    pub nodes: Vec<Node>,
    /// Extracted edges (calls, contains, defines, etc.).
    pub edges: Vec<Edge>,
    /// Import/include statements.
    pub imports: Vec<ImportInfo>,
    /// Function calls.
    pub calls: Vec<CallInfo>,
    /// Variable assignments.
    pub assignments: Vec<AssignInfo>,
    /// Extern/FFI declarations (for cross-language analysis).
    pub externs: Vec<ExternInfo>,
}

impl ExtractResult {
    /// Creates a new empty `ExtractResult` for the given file and language.
    #[must_use]
    pub fn new(file_path: impl Into<String>, language: Language) -> Self {
        Self {
            file_path: file_path.into(),
            language,
            nodes: Vec::new(),
            edges: Vec::new(),
            imports: Vec::new(),
            calls: Vec::new(),
            assignments: Vec::new(),
            externs: Vec::new(),
        }
    }

    /// Returns `true` if no symbols, edges, or records were extracted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
            && self.edges.is_empty()
            && self.imports.is_empty()
            && self.calls.is_empty()
            && self.assignments.is_empty()
            && self.externs.is_empty()
    }
}

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
/// to the language-specific [`Extractor`] (via [`get_extractor`](super::get_extractor))
/// to produce an [`ExtractResult`].
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
    let extractor = super::get_extractor(language);
    extractor.extract(&source, &path.display().to_string(), project)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EdgeType, NodeLabel};

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
        };
        assert_eq!(result.file_path, "test.rs");
        assert_eq!(result.language, Language::Rust);
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.edges.len(), 1);
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.calls.len(), 1);
        assert_eq!(result.assignments.len(), 1);
        assert_eq!(result.externs.len(), 1);
        assert!(!result.is_empty());
    }

    #[test]
    fn extract_result_is_empty_when_only_some_fields_populated() {
        let mut result = ExtractResult::new("test.rs", Language::Rust);
        result.nodes.push(
            Node::builder(NodeLabel::Function, "foo", "foo")
                .build(),
        );
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
        result.nodes.push(
            Node::builder(NodeLabel::Function, "foo", "foo")
                .build(),
        );
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
        assert!(result.is_ok(), "extract_file should succeed for existing file");
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
                result.nodes.push(
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
        let ext = DummyExtractor { lang: Language::Rust };
        assert_eq!(ext.language(), Language::Rust);
    }

    #[test]
    fn extractor_extract_returns_result_with_nodes() {
        let ext = DummyExtractor { lang: Language::Rust };
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
        let ext = DummyExtractor { lang: Language::Python };
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
        let ext = DummyExtractor { lang: Language::Rust };
        assert_dyn_extractor_send_sync(&ext);
    }

    #[test]
    fn extractor_can_be_used_as_trait_object() {
        let extractors: Vec<Box<dyn Extractor>> = vec![
            Box::new(DummyExtractor { lang: Language::Rust }),
            Box::new(DummyExtractor { lang: Language::Python }),
        ];
        assert_eq!(extractors.len(), 2);
        assert_eq!(extractors[0].language(), Language::Rust);
        assert_eq!(extractors[1].language(), Language::Python);

        let result = extractors[0].extract("x", "a.rs", "p").unwrap();
        assert_eq!(result.nodes.len(), 1);
    }
}
