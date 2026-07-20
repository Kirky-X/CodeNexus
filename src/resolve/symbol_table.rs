// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! File-level and project-level symbol tables (resolve/symbol_table.rs).
//!
//! The file-level table indexes symbols by name within a single file. The
//! project-level table aggregates file tables and provides global lookup
//! across all files.

use std::collections::{HashMap, HashSet};

use crate::ir::ExtractResult;
use crate::model::{Language, NodeLabel};

use super::includes_graph::IncludesGraph;

/// A single entry in a symbol table, representing one definition.
#[derive(Debug, Clone)]
pub struct SymbolEntry {
    /// The simple (unqualified) name of the symbol.
    pub name: String,
    /// The fully-qualified name of the symbol.
    pub qn: String,
    /// The node label (function, class, variable, etc.).
    pub label: NodeLabel,
    /// The source file path where the symbol is defined.
    pub file_path: String,
    /// The project this symbol belongs to.
    pub project: String,
    /// The function/method signature, if applicable.
    pub signature: Option<String>,
    /// The source language, if applicable.
    pub language: Option<Language>,
    /// Whether the symbol is exported (public API).
    pub is_exported: bool,
}

impl SymbolEntry {
    /// Creates a new `SymbolEntry` with the required fields.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        qn: impl Into<String>,
        label: NodeLabel,
        file_path: impl Into<String>,
        project: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            qn: qn.into(),
            label,
            file_path: file_path.into(),
            project: project.into(),
            signature: None,
            language: None,
            is_exported: false,
        }
    }

    /// Sets the signature.
    #[must_use]
    pub fn with_signature(mut self, signature: impl Into<String>) -> Self {
        self.signature = Some(signature.into());
        self
    }

    /// Sets the signature from an `Option`.
    #[must_use]
    pub fn with_signature_opt(mut self, signature: Option<String>) -> Self {
        self.signature = signature;
        self
    }

    /// Sets the language.
    #[must_use]
    pub fn with_language(mut self, language: Language) -> Self {
        self.language = Some(language);
        self
    }

    /// Sets the exported flag.
    #[must_use]
    pub fn with_exported(mut self, is_exported: bool) -> Self {
        self.is_exported = is_exported;
        self
    }
}

/// A symbol table for a single source file.
///
/// Maps symbol names to one or more entries (a name may be defined multiple
/// times in the same file, e.g. overloads).
#[derive(Debug, Clone, Default)]
pub struct FileSymbolTable {
    symbols: HashMap<String, Vec<SymbolEntry>>,
}

impl FileSymbolTable {
    /// Creates an empty file symbol table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a symbol entry to the table.
    pub fn add(&mut self, entry: SymbolEntry) {
        self.symbols
            .entry(entry.name.clone())
            .or_default()
            .push(entry);
    }

    /// Returns all entries matching the given name.
    #[must_use]
    pub fn lookup(&self, name: &str) -> Vec<&SymbolEntry> {
        self.symbols
            .get(name)
            .map(|v| v.iter().collect())
            .unwrap_or_default()
    }

    /// Returns the first entry matching the given name, or `None`.
    #[must_use]
    pub fn lookup_exact(&self, name: &str) -> Option<&SymbolEntry> {
        self.symbols.get(name).and_then(|v| v.first())
    }

    /// Returns an iterator over all symbol entries in the table.
    pub fn all_symbols(&self) -> impl Iterator<Item = &SymbolEntry> {
        self.symbols.values().flat_map(|v| v.iter())
    }

    /// Returns the total number of symbol entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.symbols.values().map(Vec::len).sum()
    }

    /// Returns `true` if the table contains no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A project-level symbol table aggregating multiple file tables.
///
/// Provides both file-scoped lookup ([`lookup_in_file`]) and global lookup
/// ([`lookup`]) across all files.
///
/// [`lookup_in_file`]: ProjectSymbolTable::lookup_in_file
/// [`lookup`]: ProjectSymbolTable::lookup
#[derive(Debug, Clone, Default)]
pub struct ProjectSymbolTable {
    files: HashMap<String, FileSymbolTable>,
    global_symbols: HashMap<String, Vec<SymbolEntry>>,
    /// B13 fix: QN set of symbols that are re-exported via `pub use` /
    /// `export ... from`. Populated by [`populate_reexport_targets`] from
    /// `ExtractResult.imports`. Used by `CallResolver::resolve_call_internal`
    /// step 3 to prefer re-export targets over other exported entries with
    /// the same simple name.
    ///
    /// # Background
    ///
    /// `pub use cli::run` in `src/lib.rs` re-exports `cli::run` as the
    /// crate's public API. When `main.rs` calls `calnexus::run()`, the
    /// parser emits `callee_name = "run"` (scoped_identifier only keeps
    /// the last segment). `lookup_exported("run")` then returns multiple
    /// matches: `batch::run` (alphabetically first) and `cli::run`. Without
    /// this set, `.first()` picks `batch::run`, breaking the
    /// `main → cli::run` call edge and causing 100% false positives in
    /// dead_code analysis on `cli.rs` (CalNexus regression).
    ///
    /// [`populate_reexport_targets`]: ProjectSymbolTable::populate_reexport_targets
    reexport_target_qns: HashSet<String>,
}

impl ProjectSymbolTable {
    /// Creates an empty project symbol table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a file symbol table to the project.
    ///
    /// All entries in the file table are also registered in the global index.
    ///
    /// Entries are sorted by qualified name before insertion to guarantee
    /// deterministic iteration order inside `global_symbols[name]`. Without
    /// this, `FileSymbolTable::all_symbols()` iterates a `HashMap` whose order
    /// is randomized per process (SipHash seed), making `lookup_exact().first()`
    /// and any caller relying on `Vec` order non-deterministic across runs.
    /// See B12 fix in `tools/verification/results/triage.md`.
    pub fn add_file_table(&mut self, file_path: &str, table: FileSymbolTable) {
        // Collect clones first so we can move `table` afterwards.
        let mut entries: Vec<SymbolEntry> = table.all_symbols().cloned().collect();
        entries.sort_by(|a, b| a.qn.cmp(&b.qn));
        for entry in entries {
            self.global_symbols
                .entry(entry.name.clone())
                .or_default()
                .push(entry);
        }
        self.files.insert(file_path.to_string(), table);
    }

    /// Adds a single symbol to both the global index and its file table.
    pub fn add_symbol(&mut self, entry: SymbolEntry) {
        let file_path = entry.file_path.clone();
        self.global_symbols
            .entry(entry.name.clone())
            .or_default()
            .push(entry.clone());
        self.files.entry(file_path).or_default().add(entry);
    }

    /// Global lookup: returns all entries matching the name across all files.
    ///
    /// Results are sorted by qualified name so callers using `.first()`
    /// (e.g. `CallResolver::resolve_call_internal` steps 2/3) observe the
    /// same entry across runs. The `global_symbols` `HashMap` itself iterates
    /// in random order per process, but `add_file_table` already sorts entries
    /// on insertion — sorting here is a defensive guarantee for entries added
    /// via `add_symbol` or any other future mutation path. See B12 fix.
    ///
    /// The `len() > 1` short-circuit avoids the `sort_by` call entirely on the
    /// common K=1 path (single-definition name), which is the overwhelming
    /// majority in real codebases. `sort_by` on a 0/1-element `Vec` is already
    /// a no-op in the standard library, but skipping the call saves the
    /// function-call overhead on this hot path.
    #[must_use]
    pub fn lookup(&self, name: &str) -> Vec<&SymbolEntry> {
        let mut v: Vec<&SymbolEntry> = self
            .global_symbols
            .get(name)
            .map(|v| v.iter().collect())
            .unwrap_or_default();
        if v.len() > 1 {
            v.sort_by(|a, b| a.qn.cmp(&b.qn));
        }
        v
    }

    /// Returns the first entry matching the name across all files, or `None`.
    #[must_use]
    pub fn lookup_exact(&self, name: &str) -> Option<&SymbolEntry> {
        self.global_symbols.get(name).and_then(|v| v.first())
    }

    /// File-scoped lookup: returns all entries matching the name in the
    /// specified file.
    #[must_use]
    pub fn lookup_in_file(&self, file_path: &str, name: &str) -> Vec<&SymbolEntry> {
        self.files
            .get(file_path)
            .map(|t| t.lookup(name))
            .unwrap_or_default()
    }

    /// Returns all exported entries matching the name across all files.
    #[must_use]
    pub fn lookup_exported(&self, name: &str) -> Vec<&SymbolEntry> {
        self.lookup(name)
            .into_iter()
            .filter(|e| e.is_exported)
            .collect()
    }

    /// Returns exported entries matching `name` that are in scope via
    /// `#include` relationships (BUG-C4 fix, v0.3.0).
    ///
    /// A symbol in file B is a valid resolution target for a call in file A
    /// if A `#include`s B (directly or transitively). This method:
    /// 1. Computes the set of files reachable from `calling_file` via
    ///    `includes_graph.reachable_from()` (includes `calling_file` itself).
    /// 2. Filters `lookup(name)` to entries that are `is_exported` AND whose
    ///    `file_path` is in the reachable set.
    ///
    /// # Arguments
    ///
    /// * `name` - The simple name of the symbol to look up.
    /// * `calling_file` - The file path of the caller (absolute, matching
    ///   `SymbolEntry::file_path` format).
    /// * `includes_graph` - The C++ `#include` graph (from `ResolvePhase`).
    ///
    /// # Returns
    ///
    /// Vector of references to matching `SymbolEntry` instances. Empty if no
    /// exported entry is in scope.
    ///
    /// # Note
    ///
    /// For non-C++ languages or when `includes_graph` is empty, this method
    /// returns the same results as [`lookup_exported`](Self::lookup_exported)
    /// only if `calling_file` itself contains the symbol. For cross-file
    /// resolution in non-C++ languages, use [`lookup_exported`](Self::lookup_exported)
    /// directly (import-based scoping is handled by `CallResolver`).
    #[must_use]
    pub fn lookup_exported_in_scope(
        &self,
        name: &str,
        calling_file: &str,
        includes_graph: &IncludesGraph,
    ) -> Vec<&SymbolEntry> {
        let mut reachable = HashSet::new();
        includes_graph.fill_reachable_from(calling_file, &mut reachable);
        self.lookup(name)
            .into_iter()
            .filter(|e| e.is_exported && reachable.contains(e.file_path.as_str()))
            .collect()
    }

    /// Returns all symbol entries across all files.
    #[must_use]
    pub fn all_symbols(&self) -> Vec<&SymbolEntry> {
        self.global_symbols
            .values()
            .flat_map(|v| v.iter())
            .collect()
    }

    /// Returns the number of file tables.
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Returns the total number of symbol entries.
    #[must_use]
    pub fn symbol_count(&self) -> usize {
        self.global_symbols.values().map(Vec::len).sum()
    }

    /// B13 fix: Marks a symbol (by fully-qualified name) as a re-export target.
    ///
    /// Called by [`populate_reexport_targets`](Self::populate_reexport_targets)
    /// during `build_symbol_table`. Re-export targets are preferred by
    /// `CallResolver::resolve_call_internal` step 3 when multiple exported
    /// entries share the same simple name.
    pub fn mark_reexport_target(&mut self, qn: String) {
        self.reexport_target_qns.insert(qn);
    }

    /// B13 fix: Returns `true` if the symbol (by QN) is a re-export target.
    ///
    /// Used by `CallResolver::resolve_call_internal` step 3 to prefer
    /// re-export targets over alphabetically-first exported entries.
    #[must_use]
    pub fn is_reexport_target(&self, qn: &str) -> bool {
        self.reexport_target_qns.contains(qn)
    }

    /// B13 fix: Populates [`reexport_target_qns`](Self::reexport_target_qns)
    /// from `pub use` / `export ... from` statements in `ExtractResult.imports`.
    ///
    /// For each re-export (`is_reexport = true`) with non-empty
    /// `imported_names`, resolves the `source_file` to a target file path,
    /// then looks up each `imported_names[i]` in that file's symbol table
    /// and marks the matching entry's QN as a re-export target.
    ///
    /// # Arguments
    ///
    /// * `results` - The extraction results containing import information.
    ///
    /// # Resolution strategy (deterministic — Rule 5)
    ///
    /// Only Rust module paths are resolved here (the CalNexus regression
    /// case). Other languages' re-exports are handled by the existing
    /// `ImportResolver` REEXPORTS edge creation, which feeds into
    /// dead_code's B7 seed via Cypher query — no symbol-table annotation
    /// needed. File path matching uses suffix match with path boundary
    /// check to bridge absolute (production `file_path`) vs relative
    /// (candidate) gap, matching `imports.rs::find_best_suffix_match`.
    pub(crate) fn populate_reexport_targets(&mut self, results: &[ExtractResult]) {
        // B13 fix: pub(crate) ensures only `orchestrator::build_symbol_table`
        // calls this, which guarantees the call happens AFTER all file tables
        // are added. When `files` is empty (results have imports but no nodes),
        // suffix match simply misses — no re-export targets are populated,
        // which is correct behavior for that edge case.
        for result in results {
            for import in &result.imports {
                if !import.is_reexport || import.imported_names.is_empty() {
                    continue;
                }
                let candidates = resolve_reexport_target_file_candidates(&import.source_file);
                if candidates.is_empty() {
                    continue;
                }
                // suffix match against `files` keys (handles absolute vs
                // relative file paths). Pick the LONGEST matching candidate
                // for determinism (most specific module path).
                let matched_file = self
                    .files
                    .keys()
                    .filter_map(|fp| {
                        let fp_norm = fp.replace('\\', "/");
                        candidates
                            .iter()
                            .rev()
                            .find(|c| file_path_matches_candidate(&fp_norm, c))
                            .map(|c| (c.len(), fp.clone()))
                    })
                    .max_by_key(|(len, _)| *len)
                    .map(|(_, fp)| fp);
                let Some(target_file) = matched_file else {
                    continue;
                };
                let Some(file_table) = self.files.get(&target_file) else {
                    continue;
                };
                for name in &import.imported_names {
                    if let Some(entry) = file_table.lookup_exact(name) {
                        self.reexport_target_qns.insert(entry.qn.clone());
                    }
                }
            }
        }
    }
}

/// B13 fix: Generates candidate relative file paths for a re-export's
/// `source_file`.
///
/// Only handles Rust module paths (`crate::`, `self::`, `super::`, and
/// unprefixed `cli::run` — the last is the CalNexus regression case where
/// `pub use cli::run` in `src/lib.rs` produces `source_file = "cli::run"`).
/// Non-Rust specifiers (file paths, external modules) return empty — they
/// are handled by `ImportResolver`'s REEXPORTS edge creation and the
/// dead_code B7 Cypher seed.
///
/// # Candidates (tried in order, longest first for suffix-match priority)
///
/// For `crate::a::b` / `a::b` (path = "a/b"):
/// 1. `src/a/b.rs`, `src/a/b/mod.rs` (full path as module)
/// 2. `src/a.rs`, `src/a/mod.rs` (last component is symbol name)
fn resolve_reexport_target_file_candidates(source_file: &str) -> Vec<String> {
    let path = if let Some(p) = source_file.strip_prefix("crate::") {
        p
    } else if let Some(p) = source_file.strip_prefix("self::") {
        p
    } else if let Some(p) = source_file.strip_prefix("super::") {
        p
    } else if source_file.contains("::") && !source_file.contains('/') && !source_file.contains('.')
    {
        source_file
    } else {
        return Vec::new();
    };
    // Defense-in-depth: reject path traversal attempts from tree-sitter input.
    // Candidates are only used for string matching against HashMap keys (never
    // opened as files), but reject early to keep the invariant explicit.
    if path.contains("..") {
        return Vec::new();
    }
    let path = path.replace("::", "/");
    let mut candidates = Vec::new();
    candidates.push(format!("src/{path}.rs"));
    candidates.push(format!("src/{path}/mod.rs"));
    if let Some((parent, _)) = path.rsplit_once('/') {
        candidates.push(format!("src/{parent}.rs"));
        candidates.push(format!("src/{parent}/mod.rs"));
    }
    candidates
}

/// B13 fix: Checks if `file_path` (normalised, possibly absolute) matches
/// `candidate` (relative, e.g. `src/cli.rs`).
///
/// Direct match or suffix match with path boundary check (the char before
/// the suffix must be `/`). Mirrors `imports.rs::find_best_suffix_match`
/// logic without cross-module dependency.
fn file_path_matches_candidate(file_path_norm: &str, candidate: &str) -> bool {
    if file_path_norm == candidate {
        return true;
    }
    // strip_suffix returns Some only when len >= candidate.len(); the equal
    // case is already handled above, so no extra filter needed.
    if let Some(prefix) = file_path_norm.strip_suffix(candidate) {
        return prefix.ends_with('/');
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(name: &str, qn: &str, file: &str) -> SymbolEntry {
        SymbolEntry::new(name, qn, NodeLabel::Function, file, "proj")
    }

    fn make_exported_entry(name: &str, qn: &str, file: &str) -> SymbolEntry {
        make_entry(name, qn, file).with_exported(true)
    }

    // --- SymbolEntry ---

    #[test]
    fn symbol_entry_new_sets_required_fields() {
        let entry = SymbolEntry::new(
            "foo",
            "proj.src.foo",
            NodeLabel::Function,
            "src/main.rs",
            "proj",
        );
        assert_eq!(entry.name, "foo");
        assert_eq!(entry.qn, "proj.src.foo");
        assert_eq!(entry.label, NodeLabel::Function);
        assert_eq!(entry.file_path, "src/main.rs");
        assert_eq!(entry.project, "proj");
        assert!(entry.signature.is_none());
        assert!(entry.language.is_none());
        assert!(!entry.is_exported);
    }

    #[test]
    fn symbol_entry_with_builders() {
        let entry = SymbolEntry::new("foo", "qn", NodeLabel::Function, "f", "p")
            .with_signature("fn foo()")
            .with_language(Language::Rust)
            .with_exported(true);
        assert_eq!(entry.signature.as_deref(), Some("fn foo()"));
        assert_eq!(entry.language, Some(Language::Rust));
        assert!(entry.is_exported);
    }

    #[test]
    fn symbol_entry_with_signature_opt_some() {
        let entry = SymbolEntry::new("foo", "qn", NodeLabel::Function, "f", "p")
            .with_signature_opt(Some("fn foo()".to_string()));
        assert_eq!(entry.signature.as_deref(), Some("fn foo()"));
    }

    #[test]
    fn symbol_entry_with_signature_opt_none() {
        let entry =
            SymbolEntry::new("foo", "qn", NodeLabel::Function, "f", "p").with_signature_opt(None);
        assert!(entry.signature.is_none());
    }

    #[test]
    fn symbol_entry_clone_is_equal() {
        let entry = make_entry("foo", "proj.foo", "f.rs");
        let cloned = entry.clone();
        assert_eq!(entry.name, cloned.name);
        assert_eq!(entry.qn, cloned.qn);
        assert_eq!(entry.label, cloned.label);
    }

    #[test]
    fn symbol_entry_accepts_string_and_str() {
        let entry = SymbolEntry::new(
            String::from("foo"),
            String::from("qn"),
            NodeLabel::Function,
            String::from("f"),
            String::from("p"),
        );
        assert_eq!(entry.name, "foo");
        assert_eq!(entry.file_path, "f");
    }

    #[test]
    fn symbol_entry_debug_contains_name() {
        let entry = make_entry("foo", "proj.foo", "f.rs");
        let debug = format!("{entry:?}");
        assert!(debug.contains("foo"));
    }

    // --- FileSymbolTable ---

    #[test]
    fn file_table_new_is_empty() {
        let table = FileSymbolTable::new();
        assert!(table.is_empty());
        assert_eq!(table.len(), 0);
    }

    #[test]
    fn file_table_add_and_lookup() {
        let mut table = FileSymbolTable::new();
        table.add(make_entry("foo", "proj.foo", "f.rs"));
        let results = table.lookup("foo");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].qn, "proj.foo");
    }

    #[test]
    fn file_table_multiple_entries_same_name() {
        let mut table = FileSymbolTable::new();
        table.add(make_entry("foo", "proj.foo1", "f.rs"));
        table.add(make_entry("foo", "proj.foo2", "f.rs"));
        let results = table.lookup("foo");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn file_table_lookup_exact_returns_first() {
        let mut table = FileSymbolTable::new();
        table.add(make_entry("foo", "proj.foo1", "f.rs"));
        table.add(make_entry("foo", "proj.foo2", "f.rs"));
        let result = table.lookup_exact("foo");
        assert!(result.is_some());
        assert_eq!(result.unwrap().qn, "proj.foo1");
    }

    #[test]
    fn file_table_lookup_exact_returns_none_if_not_found() {
        let table = FileSymbolTable::new();
        assert!(table.lookup_exact("missing").is_none());
    }

    #[test]
    fn file_table_lookup_returns_empty_if_not_found() {
        let mut table = FileSymbolTable::new();
        table.add(make_entry("foo", "proj.foo", "f.rs"));
        assert!(table.lookup("missing").is_empty());
    }

    #[test]
    fn file_table_all_symbols_iterates_all() {
        let mut table = FileSymbolTable::new();
        table.add(make_entry("a", "proj.a", "f.rs"));
        table.add(make_entry("b", "proj.b", "f.rs"));
        table.add(make_entry("a", "proj.a2", "f.rs"));
        let count = table.all_symbols().count();
        assert_eq!(count, 3);
    }

    #[test]
    fn file_table_len_counts_all_entries() {
        let mut table = FileSymbolTable::new();
        table.add(make_entry("a", "proj.a", "f.rs"));
        table.add(make_entry("a", "proj.a2", "f.rs"));
        table.add(make_entry("b", "proj.b", "f.rs"));
        assert_eq!(table.len(), 3);
    }

    #[test]
    fn file_table_default_is_empty() {
        let table = FileSymbolTable::default();
        assert!(table.is_empty());
    }

    #[test]
    fn file_table_clone_preserves_entries() {
        let mut table = FileSymbolTable::new();
        table.add(make_entry("foo", "proj.foo", "f.rs"));
        let cloned = table.clone();
        assert_eq!(cloned.len(), 1);
        assert_eq!(cloned.lookup("foo").len(), 1);
    }

    #[test]
    fn file_table_all_symbols_returns_correct_entries() {
        let mut table = FileSymbolTable::new();
        table.add(make_entry("a", "proj.a", "f.rs"));
        table.add(make_entry("b", "proj.b", "f.rs"));
        let qns: Vec<&str> = table.all_symbols().map(|e| e.qn.as_str()).collect();
        assert!(qns.contains(&"proj.a"));
        assert!(qns.contains(&"proj.b"));
    }

    // --- ProjectSymbolTable ---

    #[test]
    fn project_table_new_is_empty() {
        let table = ProjectSymbolTable::new();
        assert_eq!(table.file_count(), 0);
        assert_eq!(table.symbol_count(), 0);
    }

    #[test]
    fn project_table_add_file_table() {
        let mut project = ProjectSymbolTable::new();
        let mut file_table = FileSymbolTable::new();
        file_table.add(make_entry("foo", "proj.foo", "a.rs"));
        project.add_file_table("a.rs", file_table);
        assert_eq!(project.file_count(), 1);
        assert_eq!(project.symbol_count(), 1);
    }

    #[test]
    fn project_table_add_symbol() {
        let mut project = ProjectSymbolTable::new();
        project.add_symbol(make_entry("foo", "proj.foo", "a.rs"));
        assert_eq!(project.symbol_count(), 1);
        assert_eq!(project.file_count(), 1);
    }

    #[test]
    fn project_table_lookup_global() {
        let mut project = ProjectSymbolTable::new();
        project.add_symbol(make_entry("foo", "proj.foo", "a.rs"));
        let results = project.lookup("foo");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].qn, "proj.foo");
    }

    #[test]
    fn project_table_lookup_returns_empty_if_not_found() {
        let project = ProjectSymbolTable::new();
        assert!(project.lookup("missing").is_empty());
    }

    #[test]
    fn project_table_lookup_in_file() {
        let mut project = ProjectSymbolTable::new();
        project.add_symbol(make_entry("foo", "proj.foo", "a.rs"));
        project.add_symbol(make_entry("bar", "proj.bar", "b.rs"));
        let results = project.lookup_in_file("a.rs", "foo");
        assert_eq!(results.len(), 1);
        let results = project.lookup_in_file("a.rs", "bar");
        assert!(results.is_empty());
    }

    #[test]
    fn project_table_lookup_in_file_nonexistent_returns_empty() {
        let project = ProjectSymbolTable::new();
        assert!(project.lookup_in_file("missing.rs", "foo").is_empty());
    }

    #[test]
    fn project_table_lookup_exported() {
        let mut project = ProjectSymbolTable::new();
        project.add_symbol(make_exported_entry("foo", "proj.foo1", "a.rs"));
        project.add_symbol(make_entry("foo", "proj.foo2", "b.rs"));
        let exported = project.lookup_exported("foo");
        assert_eq!(exported.len(), 1);
        assert_eq!(exported[0].qn, "proj.foo1");
    }

    #[test]
    fn project_table_lookup_exported_returns_empty_if_none_exported() {
        let mut project = ProjectSymbolTable::new();
        project.add_symbol(make_entry("foo", "proj.foo", "a.rs"));
        assert!(project.lookup_exported("foo").is_empty());
    }

    #[test]
    fn project_table_lookup_exported_returns_empty_if_not_found() {
        let project = ProjectSymbolTable::new();
        assert!(project.lookup_exported("missing").is_empty());
    }

    // --- lookup_exported_in_scope (BUG-C4 fix, v0.3.0) ---

    use crate::resolve::includes_graph::IncludesGraph;

    #[test]
    fn lookup_exported_in_scope_filters_by_include() {
        // Spec Red test: file A includes B, B has `fn foo()` exported,
        // C also has `fn foo()` exported. lookup_exported_in_scope("foo", "A",
        // &graph) returns ONLY B's entry, not C's.
        let mut project = ProjectSymbolTable::new();
        project.add_symbol(make_exported_entry("foo", "proj.B.foo", "/abs/B.h"));
        project.add_symbol(make_exported_entry("foo", "proj.C.foo", "/abs/C.h"));

        let mut graph = IncludesGraph::new();
        graph.add_include("/abs/A.cpp", "/abs/B.h");
        // A does NOT include C.

        let results = project.lookup_exported_in_scope("foo", "/abs/A.cpp", &graph);
        assert_eq!(results.len(), 1, "only B's foo should be in scope");
        assert_eq!(results[0].qn, "proj.B.foo");
    }

    #[test]
    fn lookup_exported_in_scope_includes_same_file() {
        // A file is always reachable from itself: a function defined in the
        // same file as the caller is a valid resolution target.
        let mut project = ProjectSymbolTable::new();
        project.add_symbol(make_exported_entry("foo", "proj.A.foo", "/abs/A.cpp"));

        let graph = IncludesGraph::new(); // empty — no includes
        let results = project.lookup_exported_in_scope("foo", "/abs/A.cpp", &graph);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].qn, "proj.A.foo");
    }

    #[test]
    fn lookup_exported_in_scope_transitive_include() {
        // A includes B, B includes C. C has `fn foo()` exported.
        // Transitive reachability: lookup from A should find C's foo.
        let mut project = ProjectSymbolTable::new();
        project.add_symbol(make_exported_entry("foo", "proj.C.foo", "/abs/C.h"));

        let mut graph = IncludesGraph::new();
        graph.add_include("/abs/A.cpp", "/abs/B.h");
        graph.add_include("/abs/B.h", "/abs/C.h");

        let results = project.lookup_exported_in_scope("foo", "/abs/A.cpp", &graph);
        assert_eq!(results.len(), 1, "transitive include should reach C");
        assert_eq!(results[0].qn, "proj.C.foo");
    }

    #[test]
    fn lookup_exported_in_scope_excludes_unreachable() {
        // B has `fn foo()` exported, but A does NOT include B (directly or
        // transitively). lookup from A should return empty.
        let mut project = ProjectSymbolTable::new();
        project.add_symbol(make_exported_entry("foo", "proj.B.foo", "/abs/B.h"));

        let graph = IncludesGraph::new(); // A does not include B
        let results = project.lookup_exported_in_scope("foo", "/abs/A.cpp", &graph);
        assert!(results.is_empty(), "unreachable file should be excluded");
    }

    #[test]
    fn lookup_exported_in_scope_filters_non_exported() {
        // B has `fn foo()` but is_exported=false. A includes B.
        // lookup should return empty (only exported symbols qualify).
        let mut project = ProjectSymbolTable::new();
        project.add_symbol(make_entry("foo", "proj.B.foo", "/abs/B.h")); // not exported

        let mut graph = IncludesGraph::new();
        graph.add_include("/abs/A.cpp", "/abs/B.h");

        let results = project.lookup_exported_in_scope("foo", "/abs/A.cpp", &graph);
        assert!(results.is_empty(), "non-exported symbol should be excluded");
    }

    #[test]
    fn lookup_exported_in_scope_returns_empty_if_name_not_found() {
        let mut project = ProjectSymbolTable::new();
        project.add_symbol(make_exported_entry("foo", "proj.B.foo", "/abs/B.h"));

        let mut graph = IncludesGraph::new();
        graph.add_include("/abs/A.cpp", "/abs/B.h");

        let results = project.lookup_exported_in_scope("missing", "/abs/A.cpp", &graph);
        assert!(results.is_empty());
    }

    #[test]
    fn lookup_exported_in_scope_multiple_in_scope_entries() {
        // A includes B and C. Both B and C have `fn foo()` exported.
        // lookup from A should return both entries.
        let mut project = ProjectSymbolTable::new();
        project.add_symbol(make_exported_entry("foo", "proj.B.foo", "/abs/B.h"));
        project.add_symbol(make_exported_entry("foo", "proj.C.foo", "/abs/C.h"));
        // D also has foo but A does not include D.
        project.add_symbol(make_exported_entry("foo", "proj.D.foo", "/abs/D.h"));

        let mut graph = IncludesGraph::new();
        graph.add_include("/abs/A.cpp", "/abs/B.h");
        graph.add_include("/abs/A.cpp", "/abs/C.h");

        let results = project.lookup_exported_in_scope("foo", "/abs/A.cpp", &graph);
        assert_eq!(results.len(), 2, "B and C in scope, D excluded");
        let qns: Vec<&str> = results.iter().map(|e| e.qn.as_str()).collect();
        assert!(qns.contains(&"proj.B.foo"));
        assert!(qns.contains(&"proj.C.foo"));
        assert!(!qns.contains(&"proj.D.foo"));
    }

    #[test]
    fn project_table_all_symbols() {
        let mut project = ProjectSymbolTable::new();
        project.add_symbol(make_entry("a", "proj.a", "f1.rs"));
        project.add_symbol(make_entry("b", "proj.b", "f2.rs"));
        project.add_symbol(make_entry("a", "proj.a2", "f3.rs"));
        let all = project.all_symbols();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn project_table_file_count() {
        let mut project = ProjectSymbolTable::new();
        project.add_symbol(make_entry("a", "proj.a", "f1.rs"));
        project.add_symbol(make_entry("b", "proj.b", "f2.rs"));
        project.add_symbol(make_entry("c", "proj.c", "f1.rs"));
        assert_eq!(project.file_count(), 2);
    }

    #[test]
    fn project_table_symbol_count() {
        let mut project = ProjectSymbolTable::new();
        project.add_symbol(make_entry("a", "proj.a", "f1.rs"));
        project.add_symbol(make_entry("a", "proj.a2", "f1.rs"));
        project.add_symbol(make_entry("b", "proj.b", "f2.rs"));
        assert_eq!(project.symbol_count(), 3);
    }

    #[test]
    fn project_table_cross_file_lookup() {
        let mut project = ProjectSymbolTable::new();
        project.add_symbol(make_entry("foo", "proj.a.foo", "a.rs"));
        project.add_symbol(make_entry("foo", "proj.b.foo", "b.rs"));
        let results = project.lookup("foo");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn project_table_add_file_table_populates_global() {
        let mut project = ProjectSymbolTable::new();
        let mut file_table = FileSymbolTable::new();
        file_table.add(make_entry("foo", "proj.foo", "a.rs"));
        file_table.add(make_entry("bar", "proj.bar", "a.rs"));
        project.add_file_table("a.rs", file_table);
        // Global lookup should find both symbols.
        assert_eq!(project.lookup("foo").len(), 1);
        assert_eq!(project.lookup("bar").len(), 1);
        assert_eq!(project.symbol_count(), 2);
    }

    #[test]
    fn project_table_add_file_table_populates_file_lookup() {
        let mut project = ProjectSymbolTable::new();
        let mut file_table = FileSymbolTable::new();
        file_table.add(make_entry("foo", "proj.foo", "a.rs"));
        project.add_file_table("a.rs", file_table);
        // File-scoped lookup should also work.
        assert_eq!(project.lookup_in_file("a.rs", "foo").len(), 1);
    }

    #[test]
    fn project_table_default_is_empty() {
        let table = ProjectSymbolTable::default();
        assert_eq!(table.file_count(), 0);
        assert_eq!(table.symbol_count(), 0);
    }

    #[test]
    fn project_table_clone_preserves_data() {
        let mut project = ProjectSymbolTable::new();
        project.add_symbol(make_entry("foo", "proj.foo", "a.rs"));
        let cloned = project.clone();
        assert_eq!(cloned.symbol_count(), 1);
        assert_eq!(cloned.lookup("foo").len(), 1);
    }

    #[test]
    fn project_table_add_multiple_file_tables() {
        let mut project = ProjectSymbolTable::new();
        let mut ft1 = FileSymbolTable::new();
        ft1.add(make_entry("foo", "proj.a.foo", "a.rs"));
        let mut ft2 = FileSymbolTable::new();
        ft2.add(make_entry("bar", "proj.b.bar", "b.rs"));
        project.add_file_table("a.rs", ft1);
        project.add_file_table("b.rs", ft2);
        assert_eq!(project.file_count(), 2);
        assert_eq!(project.symbol_count(), 2);
        assert_eq!(project.lookup("foo").len(), 1);
        assert_eq!(project.lookup("bar").len(), 1);
    }

    #[test]
    fn project_table_add_symbol_creates_file_table_if_needed() {
        let mut project = ProjectSymbolTable::new();
        project.add_symbol(make_entry("foo", "proj.foo", "a.rs"));
        project.add_symbol(make_entry("bar", "proj.bar", "a.rs"));
        // Both symbols should be in the same file table.
        assert_eq!(project.file_count(), 1);
        assert_eq!(project.lookup_in_file("a.rs", "foo").len(), 1);
        assert_eq!(project.lookup_in_file("a.rs", "bar").len(), 1);
    }

    // --- populate_reexport_targets (Fix2) ---

    use crate::ir::ImportInfo;

    fn make_import_result(file_path: &str, imports: Vec<ImportInfo>) -> ExtractResult {
        let mut result = ExtractResult::new(file_path, Language::Rust);
        result.imports = imports;
        result
    }

    #[test]
    fn populate_reexport_targets_marks_reexported_function_via_pub_use() {
        // CalNexus regression scenario: `pub use cli::run` in src/lib.rs
        // should mark cli.rs::run as a re-export target.
        let mut project = ProjectSymbolTable::new();
        let mut cli_table = FileSymbolTable::new();
        cli_table.add(make_exported_entry(
            "run",
            "proj.src.cli.rs.run",
            "src/cli.rs",
        ));
        project.add_file_table("src/cli.rs", cli_table);

        let lib_result = make_import_result(
            "src/lib.rs",
            vec![ImportInfo {
                source_file: "cli::run".to_string(),
                imported_names: vec!["run".to_string()],
                line: 38,
                is_reexport: true,
            }],
        );

        project.populate_reexport_targets(&[lib_result]);

        assert!(project.is_reexport_target("proj.src.cli.rs.run"));
    }

    #[test]
    fn populate_reexport_targets_skips_non_reexport_imports() {
        // Ordinary `use cli::run` (not `pub use`) must NOT be marked.
        let mut project = ProjectSymbolTable::new();
        let mut cli_table = FileSymbolTable::new();
        cli_table.add(make_exported_entry(
            "run",
            "proj.src.cli.rs.run",
            "src/cli.rs",
        ));
        project.add_file_table("src/cli.rs", cli_table);

        let main_result = make_import_result(
            "src/main.rs",
            vec![ImportInfo {
                source_file: "cli::run".to_string(),
                imported_names: vec!["run".to_string()],
                line: 1,
                is_reexport: false,
            }],
        );

        project.populate_reexport_targets(&[main_result]);

        assert!(!project.is_reexport_target("proj.src.cli.rs.run"));
    }

    #[test]
    fn populate_reexport_targets_skips_wildcard_reexports() {
        // `pub use cli::*` (empty imported_names) cannot be precisely
        // mapped to specific symbols — skip to avoid false-positive marking.
        let mut project = ProjectSymbolTable::new();
        let mut cli_table = FileSymbolTable::new();
        cli_table.add(make_exported_entry(
            "run",
            "proj.src.cli.rs.run",
            "src/cli.rs",
        ));
        project.add_file_table("src/cli.rs", cli_table);

        let lib_result = make_import_result(
            "src/lib.rs",
            vec![ImportInfo {
                source_file: "cli::*".to_string(),
                imported_names: vec![],
                line: 38,
                is_reexport: true,
            }],
        );

        project.populate_reexport_targets(&[lib_result]);

        assert!(!project.is_reexport_target("proj.src.cli.rs.run"));
    }

    #[test]
    fn populate_reexport_targets_handles_crate_prefix() {
        // `pub use crate::cli::run` — explicit crate prefix.
        let mut project = ProjectSymbolTable::new();
        let mut cli_table = FileSymbolTable::new();
        cli_table.add(make_exported_entry(
            "run",
            "proj.src.cli.rs.run",
            "src/cli.rs",
        ));
        project.add_file_table("src/cli.rs", cli_table);

        let lib_result = make_import_result(
            "src/lib.rs",
            vec![ImportInfo {
                source_file: "crate::cli::run".to_string(),
                imported_names: vec!["run".to_string()],
                line: 1,
                is_reexport: true,
            }],
        );

        project.populate_reexport_targets(&[lib_result]);

        assert!(project.is_reexport_target("proj.src.cli.rs.run"));
    }

    #[test]
    fn populate_reexport_targets_handles_absolute_file_paths() {
        // Production file_path is absolute; candidates are relative.
        // Suffix match with path boundary check bridges the gap.
        let mut project = ProjectSymbolTable::new();
        let mut cli_table = FileSymbolTable::new();
        cli_table.add(make_exported_entry(
            "run",
            "proj.src.cli.rs.run",
            "src/cli.rs",
        ));
        project.add_file_table("/home/user/projects/CalNexus/src/cli.rs", cli_table);

        let lib_result = make_import_result(
            "/home/user/projects/CalNexus/src/lib.rs",
            vec![ImportInfo {
                source_file: "cli::run".to_string(),
                imported_names: vec!["run".to_string()],
                line: 38,
                is_reexport: true,
            }],
        );

        project.populate_reexport_targets(&[lib_result]);

        assert!(project.is_reexport_target("proj.src.cli.rs.run"));
    }

    #[test]
    fn populate_reexport_targets_no_match_does_not_mark() {
        // Unresolvable source_file (external crate) must not mark anything.
        let mut project = ProjectSymbolTable::new();
        let mut cli_table = FileSymbolTable::new();
        cli_table.add(make_exported_entry(
            "run",
            "proj.src.cli.rs.run",
            "src/cli.rs",
        ));
        project.add_file_table("src/cli.rs", cli_table);

        let lib_result = make_import_result(
            "src/lib.rs",
            vec![ImportInfo {
                source_file: "serde::Serialize".to_string(),
                imported_names: vec!["Serialize".to_string()],
                line: 1,
                is_reexport: true,
            }],
        );

        project.populate_reexport_targets(&[lib_result]);

        // `cli::run` was not marked because source_file `serde::Serialize`
        // doesn't resolve to any project file. The only candidate symbol
        // (`proj.src.cli.rs.run`) must not be in the re-export set.
        assert!(!project.is_reexport_target("proj.src.cli.rs.run"));
    }

    #[test]
    fn populate_reexport_targets_prefers_most_specific_match() {
        // When both `src/cli.rs` and `src/deep/cli.rs` exist, `pub use
        // cli::run` should match `src/cli.rs` (longest candidate match).
        let mut project = ProjectSymbolTable::new();
        let mut cli_table = FileSymbolTable::new();
        cli_table.add(make_exported_entry(
            "run",
            "proj.src.cli.rs.run",
            "src/cli.rs",
        ));
        let mut deep_cli_table = FileSymbolTable::new();
        deep_cli_table.add(make_exported_entry(
            "run",
            "proj.src.deep.cli.rs.run",
            "src/deep/cli.rs",
        ));
        project.add_file_table("src/cli.rs", cli_table);
        project.add_file_table("src/deep/cli.rs", deep_cli_table);

        let lib_result = make_import_result(
            "src/lib.rs",
            vec![ImportInfo {
                source_file: "cli::run".to_string(),
                imported_names: vec!["run".to_string()],
                line: 1,
                is_reexport: true,
            }],
        );

        project.populate_reexport_targets(&[lib_result]);

        // `cli::run` candidates are `src/cli.rs`/`src/cli/mod.rs` (per
        // `resolve_reexport_target_file_candidates`), not `src/deep/cli.rs`.
        // So only `src/cli.rs` matches, even though `src/deep/cli.rs` would
        // be a longer suffix — `max_by_key` picks among matched candidates.
        assert!(project.is_reexport_target("proj.src.cli.rs.run"));
        assert!(!project.is_reexport_target("proj.src.deep.cli.rs.run"));
    }
}
