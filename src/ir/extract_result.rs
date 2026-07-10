// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Per-file extraction output aggregating all intermediate records.

use std::collections::HashSet;

use crate::ir::types::{AssignInfo, CallInfo, ExternInfo, ImportInfo, ReadInfo, WriteInfo};
use crate::model::{Edge, Language, Node};

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
