//! Intermediate Representation (IR): shared data types.
//!
//! Defines the pure data structures produced by the parse phase and consumed
//! by the resolve phase. Placing these in a dedicated `ir` module breaks the
//! `parse ↔ resolve` bidirectional dependency: both `parse` and `resolve`
//! depend on `ir`, while `parse` additionally depends on `resolve::FqnGenerator`
//! (FQN generation is part of the parse phase per ADD §7.1).
//!
//! # Types
//!
//! - [`ImportInfo`], [`CallInfo`], [`AssignInfo`], [`ExternInfo`],
//!   [`ReadInfo`], [`WriteInfo`]: intermediate extraction records.
//! - [`ExtractResult`]: the per-file extraction output aggregating all records.

use std::collections::HashSet;

use crate::model::{Edge, Language, Node};

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

/// Information about a variable read within a function body.
///
/// Captured for later resolution of Reads edges (Function -> Variable,
/// BR-TRACE-005). `reader_qn` holds the name of the enclosing function/
/// method (resolved against the symbol table during resolution).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReadInfo {
    /// The function/method that reads the variable (function name; the
    /// resolver looks it up in the symbol table to obtain its FQN).
    pub reader_qn: Option<String>,
    /// The name of the variable being read.
    pub var_name: String,
    /// The 1-based line number.
    pub line: u32,
}

/// Information about a variable write within a function body.
///
/// Captured for later resolution of Writes edges (Function -> Variable,
/// BR-TRACE-006). `writer_qn` holds the name of the enclosing function/
/// method (resolved against the symbol table during resolution).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WriteInfo {
    /// The function/method that writes the variable (function name; the
    /// resolver looks it up in the symbol table to obtain its FQN).
    pub writer_qn: Option<String>,
    /// The name of the variable being written.
    pub var_name: String,
    /// The 1-based line number.
    pub line: u32,
}

// ---------------------------------------------------------------------------
// ExtractResult: the output of extracting symbols from a single file.
// ---------------------------------------------------------------------------

/// The result of extracting symbols from a single source file.
///
/// Produced by [`crate::parse::Extractor::extract`]. Contains definition nodes,
/// edges, and intermediate records (imports, calls, assignments, externs) used
/// by the downstream resolution phase.
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
    /// Variable reads within function bodies (BR-TRACE-005).
    pub reads: Vec<ReadInfo>,
    /// Variable writes within function bodies (BR-TRACE-006).
    pub writes: Vec<WriteInfo>,
    /// Set of `qualified_name`s already inserted into `nodes` (MED-002).
    /// Maintained by [`push_node`](Self::push_node); used by
    /// `dedupe_qn` for O(1) duplicate-FQN detection instead of an O(N)
    /// linear scan over `nodes`.
    pub seen_qns: HashSet<String>,
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
            reads: Vec::new(),
            writes: Vec::new(),
            seen_qns: HashSet::new(),
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
            && self.reads.is_empty()
            && self.writes.is_empty()
    }

    /// Pushes a node and registers its `qualified_name` in [`seen_qns`](Self::seen_qns).
    ///
    /// All extractors should use this instead of `self.nodes.push(...)` so that
    /// `dedupe_qn` can detect duplicate FQNs in O(1) (MED-002). The set is
    /// consulted by `dedupe_qn` to decide whether to append a `#L{line}`
    /// disambiguator suffix.
    pub fn push_node(&mut self, node: Node) {
        self.seen_qns.insert(node.qualified_name.clone());
        self.nodes.push(node);
    }
}
