// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Intermediate extraction records collected per file.

use crate::model::Language;

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
