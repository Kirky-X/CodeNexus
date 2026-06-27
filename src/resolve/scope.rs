// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Scope chain for nested scope resolution (resolve/scope.rs).
//!
//! A scope chain represents the nesting of scopes
//! (file -> module -> class -> function -> block). Name resolution searches
//! from the innermost scope outward.

use std::collections::HashMap;

use tree_sitter::Node;

use crate::model::{Language, NodeLabel};
use super::FqnGenerator;

/// A single scope in a scope chain.
#[derive(Debug, Clone)]
pub struct Scope {
    /// The simple (unqualified) name of this scope.
    pub name: String,
    /// The qualified name of this scope.
    pub qn: String,
    /// The node label associated with this scope.
    pub label: NodeLabel,
    /// The qualified name of the parent scope, if any.
    pub parent: Option<String>,
}

impl Scope {
    /// Creates a new scope.
    #[must_use]
    pub fn new(name: impl Into<String>, qn: impl Into<String>, label: NodeLabel) -> Self {
        Self {
            name: name.into(),
            qn: qn.into(),
            label,
            parent: None,
        }
    }

    /// Sets the parent qualified name.
    #[must_use]
    pub fn with_parent(mut self, parent: impl Into<String>) -> Self {
        self.parent = Some(parent.into());
        self
    }
}

/// A chain of nested scopes used for name resolution.
///
/// Scopes are pushed/popped as the resolver enters/leaves definitions.
/// [`ScopeChain::resolve_name`] searches from the innermost scope outward.
#[derive(Debug, Clone, Default)]
pub struct ScopeChain {
    scopes: Vec<Scope>,
}

impl ScopeChain {
    /// Creates an empty scope chain.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Pushes a new scope onto the chain.
    pub fn push(&mut self, scope: Scope) {
        self.scopes.push(scope);
    }

    /// Pops the innermost scope from the chain.
    ///
    /// Does nothing if the chain is empty.
    pub fn pop(&mut self) {
        self.scopes.pop();
    }

    /// Returns the innermost (current) scope, or `None` if the chain is empty.
    #[must_use]
    pub fn current(&self) -> Option<&Scope> {
        self.scopes.last()
    }

    /// Returns the qualified name of the innermost scope, or `None` if empty.
    #[must_use]
    pub fn current_qn(&self) -> Option<&str> {
        self.scopes.last().map(|s| s.qn.as_str())
    }

    /// Returns the number of scopes in the chain.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.scopes.len()
    }

    /// Resolves a simple name to a qualified name by searching from the
    /// innermost scope outward.
    ///
    /// Returns the qualified name of the first scope whose `name` matches,
    /// or `None` if no match is found.
    #[must_use]
    pub fn resolve_name(&self, name: &str) -> Option<String> {
        self.scopes
            .iter()
            .rev()
            .find(|s| s.name == name)
            .map(|s| s.qn.clone())
    }

    /// Returns an iterator over the scopes (outermost to innermost).
    pub fn iter(&self) -> std::slice::Iter<'_, Scope> {
        self.scopes.iter()
    }

    /// Returns `true` if the chain contains no scopes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.scopes.is_empty()
    }
}

// ---------------------------------------------------------------------------
// ScopeResolver trait + per-language implementations (Task 2.6, design.md D3)
// ---------------------------------------------------------------------------

/// Immutable context carried through the tree-sitter walk, used by
/// [`ScopeResolver`] to compute the [`Scope`] of a node.
///
/// `current_parent` is the enclosing scope name (e.g. a class name for
/// methods, a module name for nested functions) threaded from ancestor
/// scope-introducing nodes. It is the same value the old extractors stored in
/// `VisitContext::current_parent`.
pub struct ScopeContext<'a> {
    /// The source text of the file being extracted (for node-text extraction).
    pub source: &'a str,
    /// The relative file path (for FQN generation).
    pub file_path: &'a str,
    /// The project name (for FQN generation).
    pub project: &'a str,
    /// The enclosing scope name threaded from ancestors, if any.
    pub current_parent: Option<&'a str>,
}

/// Resolves the [`Scope`] introduced by a tree-sitter node.
///
/// Per-language implementations identify scope-introducing definitions
/// (functions, classes, modules, etc.) and compute their [`Scope`] (name, FQN,
/// label, parent) using the [`ScopeContext`]. Nodes that do not introduce a
/// scope return `None`.
///
/// This trait replaces the manual `current_func`/`current_parent` threading in
/// extractors (design.md D3). Extractors call the registry to obtain the scope
/// of each visited node, then thread the scope info through the walk.
///
/// # Object safety
///
/// The trait is object-safe: the lifetime `'a` on [`resolve`](ScopeResolver::resolve)
/// is a method-level lifetime parameter (not a type parameter), so
/// `dyn ScopeResolver` can be stored in a `Box`/`HashMap`.
pub trait ScopeResolver: Send + Sync {
    /// Returns the [`Scope`] this node introduces, or `None` if the node is
    /// not a scope-introducing definition.
    fn resolve<'a>(&self, node: Node<'a>, ctx: &ScopeContext<'a>) -> Option<Scope>;
}

// --- Shared helpers ---

/// Extracts the UTF-8 text of `node` from `source`.
fn node_text<'a>(node: Node<'a>, source: &'a str) -> Option<&'a str> {
    node.utf8_text(source.as_bytes()).ok()
}

/// Extracts the text of the `name` field child of `node`, if present.
fn name_field<'a>(node: Node<'a>, source: &'a str) -> Option<String> {
    node.child_by_field_name("name")
        .and_then(|n| node_text(n, source).map(String::from))
}

/// Builds a [`Scope`] with an optional parent.
fn build_scope(name: String, qn: String, label: NodeLabel, parent: Option<&str>) -> Scope {
    let mut scope = Scope::new(name, qn, label);
    if let Some(p) = parent {
        scope = scope.with_parent(p);
    }
    scope
}

/// Computes the FQN for an entity using [`FqnGenerator`].
fn make_qn(
    file_path: &str,
    name: &str,
    project: &str,
    language: Language,
    parent: Option<&str>,
) -> String {
    FqnGenerator::generate(project, file_path, name, language, parent)
}

// ---------------------------------------------------------------------------
// PythonScopeResolver
// ---------------------------------------------------------------------------

/// [`ScopeResolver`] for Python (feature `lang-python`).
///
/// Scope-introducing nodes:
/// - `function_definition` → [`NodeLabel::Function`] (or [`NodeLabel::Method`]
///   when inside a class).
/// - `class_definition` → [`NodeLabel::Class`].
#[cfg(feature = "lang-python")]
pub struct PythonScopeResolver;

#[cfg(feature = "lang-python")]
impl ScopeResolver for PythonScopeResolver {
    fn resolve<'a>(&self, node: Node<'a>, ctx: &ScopeContext<'a>) -> Option<Scope> {
        match node.kind() {
            "function_definition" => {
                let name = name_field(node, ctx.source)?;
                let label = if ctx.current_parent.is_some() {
                    NodeLabel::Method
                } else {
                    NodeLabel::Function
                };
                let qn = make_qn(ctx.file_path, &name, ctx.project, Language::Python, ctx.current_parent);
                Some(build_scope(name, qn, label, ctx.current_parent))
            }
            "class_definition" => {
                let name = name_field(node, ctx.source)?;
                let qn = make_qn(ctx.file_path, &name, ctx.project, Language::Python, ctx.current_parent);
                Some(build_scope(name, qn, NodeLabel::Class, ctx.current_parent))
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// RustScopeResolver
// ---------------------------------------------------------------------------

/// [`ScopeResolver`] for Rust (feature `lang-rust`).
///
/// Scope-introducing nodes:
/// - `function_item` → [`NodeLabel::Function`].
/// - `struct_item` → [`NodeLabel::Struct`].
/// - `enum_item` → [`NodeLabel::Enum`].
/// - `trait_item` → [`NodeLabel::Trait`].
/// - `impl_item` → [`NodeLabel::Impl`].
/// - `mod_item` → [`NodeLabel::Module`].
#[cfg(feature = "lang-rust")]
pub struct RustScopeResolver;

#[cfg(feature = "lang-rust")]
impl ScopeResolver for RustScopeResolver {
    fn resolve<'a>(&self, node: Node<'a>, ctx: &ScopeContext<'a>) -> Option<Scope> {
        match node.kind() {
            "function_item" => {
                let name = name_field(node, ctx.source)?;
                let qn = make_qn(ctx.file_path, &name, ctx.project, Language::Rust, ctx.current_parent);
                Some(build_scope(name, qn, NodeLabel::Function, ctx.current_parent))
            }
            "struct_item" => {
                let name = name_field(node, ctx.source)?;
                let qn = make_qn(ctx.file_path, &name, ctx.project, Language::Rust, ctx.current_parent);
                Some(build_scope(name, qn, NodeLabel::Struct, ctx.current_parent))
            }
            "enum_item" => {
                let name = name_field(node, ctx.source)?;
                let qn = make_qn(ctx.file_path, &name, ctx.project, Language::Rust, ctx.current_parent);
                Some(build_scope(name, qn, NodeLabel::Enum, ctx.current_parent))
            }
            "trait_item" => {
                let name = name_field(node, ctx.source)?;
                let qn = make_qn(ctx.file_path, &name, ctx.project, Language::Rust, ctx.current_parent);
                Some(build_scope(name, qn, NodeLabel::Trait, ctx.current_parent))
            }
            "impl_item" => {
                // impl blocks don't have a `name` field; use the `type` field
                // (the type being implemented) as the scope name.
                let name = node
                    .child_by_field_name("type")
                    .and_then(|n| node_text(n, ctx.source).map(String::from))?;
                let qn = make_qn(ctx.file_path, &name, ctx.project, Language::Rust, ctx.current_parent);
                Some(build_scope(name, qn, NodeLabel::Impl, ctx.current_parent))
            }
            "mod_item" => {
                let name = name_field(node, ctx.source)?;
                let qn = make_qn(ctx.file_path, &name, ctx.project, Language::Rust, ctx.current_parent);
                Some(build_scope(name, qn, NodeLabel::Module, ctx.current_parent))
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// CScopeResolver
// ---------------------------------------------------------------------------

/// [`ScopeResolver`] for C/C++ (feature `lang-c`).
///
/// Scope-introducing nodes:
/// - `function_definition` → [`NodeLabel::Function`].
///   **Note**: tree-sitter-c misparses C++ `namespace`/`class`/`struct` blocks
///   as `function_definition`. This resolver detects the misparse (the `type`
///   field is a `type_identifier` whose text is `namespace`, `class`, or
///   `struct`) and returns the appropriate label instead.
/// - `struct_specifier` → [`NodeLabel::Struct`] (when a body and name exist).
#[cfg(feature = "lang-c")]
pub struct CScopeResolver;

#[cfg(feature = "lang-c")]
impl ScopeResolver for CScopeResolver {
    fn resolve<'a>(&self, node: Node<'a>, ctx: &ScopeContext<'a>) -> Option<Scope> {
        match node.kind() {
            "function_definition" => {
                // Detect C++ namespace/class/struct blocks misparsed as
                // function_definition (tree-sitter-c quirk).
                let type_text = node
                    .child_by_field_name("type")
                    .filter(|n| n.kind() == "type_identifier")
                    .and_then(|n| node_text(n, ctx.source));
                match type_text {
                    Some("namespace") => {
                        let name = node
                            .child_by_field_name("declarator")
                            .and_then(|n| node_text(n, ctx.source).map(String::from))?;
                        let qn = make_qn(ctx.file_path, &name, ctx.project, Language::C, ctx.current_parent);
                        Some(build_scope(name, qn, NodeLabel::Namespace, ctx.current_parent))
                    }
                    Some("class") => {
                        let name = node
                            .child_by_field_name("declarator")
                            .and_then(|n| node_text(n, ctx.source).map(String::from))?;
                        let qn = make_qn(ctx.file_path, &name, ctx.project, Language::C, ctx.current_parent);
                        Some(build_scope(name, qn, NodeLabel::Class, ctx.current_parent))
                    }
                    Some("struct") => {
                        let name = node
                            .child_by_field_name("declarator")
                            .and_then(|n| node_text(n, ctx.source).map(String::from))?;
                        let qn = make_qn(ctx.file_path, &name, ctx.project, Language::C, ctx.current_parent);
                        Some(build_scope(name, qn, NodeLabel::Struct, ctx.current_parent))
                    }
                    _ => {
                        // Normal C function.
                        let name = c_function_name(node, ctx.source)?;
                        let qn = make_qn(ctx.file_path, &name, ctx.project, Language::C, ctx.current_parent);
                        Some(build_scope(name, qn, NodeLabel::Function, ctx.current_parent))
                    }
                }
            }
            "struct_specifier" => {
                // Only treat named structs with a body as scope-introducing.
                let name = name_field(node, ctx.source)?;
                let qn = make_qn(ctx.file_path, &name, ctx.project, Language::C, ctx.current_parent);
                Some(build_scope(name, qn, NodeLabel::Struct, ctx.current_parent))
            }
            _ => None,
        }
    }
}

/// Extracts the function name from a C `function_definition` node.
///
/// The name may be in the `declarator` field (a `function_declarator` whose
/// `declarator` is an `identifier`), or directly in a `declarator` child.
#[cfg(feature = "lang-c")]
fn c_function_name(node: Node, source: &str) -> Option<String> {
    let declarator = node.child_by_field_name("declarator")?;
    c_declarator_name(declarator, source)
}

/// Recursively unwraps declarator nodes (function_declarator,
/// pointer_declarator, array_declarator, parenthesized_declarator,
/// init_declarator) to find the inner identifier. Mirrors the extractor's
/// `declarator_name` logic so resolver and extractor produce identical
/// results.
#[cfg(feature = "lang-c")]
fn c_declarator_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => node_text(node, source).map(String::from),
        "function_declarator" | "pointer_declarator" | "array_declarator"
        | "parenthesized_declarator" | "init_declarator" => {
            let inner = node.child_by_field_name("declarator")?;
            c_declarator_name(inner, source)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// FortranScopeResolver
// ---------------------------------------------------------------------------

/// [`ScopeResolver`] for Fortran (feature `lang-fortran`).
///
/// Scope-introducing nodes:
/// - `module` → [`NodeLabel::Module`].
/// - `subroutine` → [`NodeLabel::Function`].
/// - `function` → [`NodeLabel::Function`].
/// - `program` → [`NodeLabel::Function`] (treated as a function).
#[cfg(feature = "lang-fortran")]
pub struct FortranScopeResolver;

#[cfg(feature = "lang-fortran")]
impl ScopeResolver for FortranScopeResolver {
    fn resolve<'a>(&self, node: Node<'a>, ctx: &ScopeContext<'a>) -> Option<Scope> {
        match node.kind() {
            "module" => {
                let name = fortran_statement_name(node, "module_statement", ctx.source)?;
                let qn = make_qn(ctx.file_path, &name, ctx.project, Language::Fortran, ctx.current_parent);
                Some(build_scope(name, qn, NodeLabel::Module, ctx.current_parent))
            }
            "subroutine" => {
                let name = fortran_statement_name(node, "subroutine_statement", ctx.source)?;
                let qn = make_qn(ctx.file_path, &name, ctx.project, Language::Fortran, ctx.current_parent);
                Some(build_scope(name, qn, NodeLabel::Function, ctx.current_parent))
            }
            "function" => {
                let name = fortran_statement_name(node, "function_statement", ctx.source)?;
                let qn = make_qn(ctx.file_path, &name, ctx.project, Language::Fortran, ctx.current_parent);
                Some(build_scope(name, qn, NodeLabel::Function, ctx.current_parent))
            }
            "program" => {
                let name = fortran_statement_name(node, "program_statement", ctx.source)?;
                let qn = make_qn(ctx.file_path, &name, ctx.project, Language::Fortran, ctx.current_parent);
                Some(build_scope(name, qn, NodeLabel::Function, ctx.current_parent))
            }
            _ => None,
        }
    }
}

/// Extracts the name from a Fortran definition node by finding its statement
/// child (e.g. `module_statement`) and looking for the `name` field or a
/// child of kind `name`/`identifier`.
#[cfg(feature = "lang-fortran")]
fn fortran_statement_name(node: Node, statement_kind: &str, source: &str) -> Option<String> {
    for i in 0..node.named_child_count() as u32 {
        if let Some(child) = node.named_child(i) {
            if child.kind() == statement_kind {
                // Try the `name` field first.
                if let Some(name_node) = child.child_by_field_name("name") {
                    if let Some(text) = node_text(name_node, source) {
                        return Some(text.to_string());
                    }
                }
                // Fall back to a named child with kind `name` or `identifier`.
                for j in 0..child.named_child_count() as u32 {
                    if let Some(grandchild) = child.named_child(j) {
                        if grandchild.kind() == "name" || grandchild.kind() == "identifier" {
                            if let Some(text) = node_text(grandchild, source) {
                                return Some(text.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// TypeScriptScopeResolver
// ---------------------------------------------------------------------------

/// [`ScopeResolver`] for TypeScript (feature `lang-typescript`).
///
/// Scope-introducing nodes:
/// - `function_declaration` | `generator_function_declaration` → [`NodeLabel::Function`].
/// - `class_declaration` → [`NodeLabel::Class`].
/// - `method_definition` → [`NodeLabel::Method`].
/// - `interface_declaration` → [`NodeLabel::Interface`].
#[cfg(feature = "lang-typescript")]
pub struct TypeScriptScopeResolver;

#[cfg(feature = "lang-typescript")]
impl ScopeResolver for TypeScriptScopeResolver {
    fn resolve<'a>(&self, node: Node<'a>, ctx: &ScopeContext<'a>) -> Option<Scope> {
        match node.kind() {
            "function_declaration" | "generator_function_declaration" => {
                let name = name_field(node, ctx.source)?;
                let qn = make_qn(ctx.file_path, &name, ctx.project, Language::TypeScript, ctx.current_parent);
                Some(build_scope(name, qn, NodeLabel::Function, ctx.current_parent))
            }
            "class_declaration" => {
                let name = name_field(node, ctx.source)?;
                let qn = make_qn(ctx.file_path, &name, ctx.project, Language::TypeScript, ctx.current_parent);
                Some(build_scope(name, qn, NodeLabel::Class, ctx.current_parent))
            }
            "method_definition" => {
                let name = name_field(node, ctx.source)?;
                let qn = make_qn(ctx.file_path, &name, ctx.project, Language::TypeScript, ctx.current_parent);
                Some(build_scope(name, qn, NodeLabel::Method, ctx.current_parent))
            }
            "interface_declaration" => {
                let name = name_field(node, ctx.source)?;
                let qn = make_qn(ctx.file_path, &name, ctx.project, Language::TypeScript, ctx.current_parent);
                Some(build_scope(name, qn, NodeLabel::Interface, ctx.current_parent))
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// ScopeResolverRegistry
// ---------------------------------------------------------------------------

/// Registry of per-language [`ScopeResolver`] implementations, dispatching by
/// [`Language`].
///
/// Constructed with all resolvers available in the current build (each
/// language is feature-gated). Call [`get`](Self::get) to obtain the resolver
/// for a specific language.
#[derive(Default)]
pub struct ScopeResolverRegistry {
    resolvers: HashMap<Language, Box<dyn ScopeResolver>>,
}

impl ScopeResolverRegistry {
    /// Creates a new registry populated with all compiled-in language resolvers.
    #[must_use]
    pub fn new() -> Self {
        let mut resolvers: HashMap<Language, Box<dyn ScopeResolver>> = HashMap::new();
        #[cfg(feature = "lang-c")]
        {
            resolvers.insert(Language::C, Box::new(CScopeResolver));
        }
        #[cfg(feature = "lang-rust")]
        {
            resolvers.insert(Language::Rust, Box::new(RustScopeResolver));
        }
        #[cfg(feature = "lang-fortran")]
        {
            resolvers.insert(Language::Fortran, Box::new(FortranScopeResolver));
        }
        #[cfg(feature = "lang-python")]
        {
            resolvers.insert(Language::Python, Box::new(PythonScopeResolver));
        }
        #[cfg(feature = "lang-typescript")]
        {
            resolvers.insert(Language::TypeScript, Box::new(TypeScriptScopeResolver));
        }
        Self { resolvers }
    }

    /// Returns the [`ScopeResolver`] for `language`, or `None` if no resolver
    /// is registered (e.g. the language's feature is not enabled).
    #[must_use]
    pub fn get(&self, language: Language) -> Option<&dyn ScopeResolver> {
        self.resolvers.get(&language).map(|b| b.as_ref())
    }

    /// Returns the number of registered resolvers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.resolvers.len()
    }

    /// Returns `true` if no resolvers are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.resolvers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_scope(name: &str, qn: &str, label: NodeLabel) -> Scope {
        Scope::new(name, qn, label)
    }

    // --- Empty chain ---

    #[test]
    fn empty_chain_current_is_none() {
        let chain = ScopeChain::new();
        assert!(chain.current().is_none());
    }

    #[test]
    fn empty_chain_current_qn_is_none() {
        let chain = ScopeChain::new();
        assert!(chain.current_qn().is_none());
    }

    #[test]
    fn empty_chain_depth_is_zero() {
        let chain = ScopeChain::new();
        assert_eq!(chain.depth(), 0);
    }

    #[test]
    fn empty_chain_is_empty() {
        let chain = ScopeChain::new();
        assert!(chain.is_empty());
    }

    #[test]
    fn empty_chain_resolve_returns_none() {
        let chain = ScopeChain::new();
        assert!(chain.resolve_name("foo").is_none());
    }

    #[test]
    fn pop_on_empty_chain_is_noop() {
        let mut chain = ScopeChain::new();
        chain.pop();
        assert_eq!(chain.depth(), 0);
    }

    // --- push / pop ---

    #[test]
    fn push_increases_depth() {
        let mut chain = ScopeChain::new();
        chain.push(make_scope("main", "proj.main", NodeLabel::Function));
        assert_eq!(chain.depth(), 1);
    }

    #[test]
    fn pop_decreases_depth() {
        let mut chain = ScopeChain::new();
        chain.push(make_scope("main", "proj.main", NodeLabel::Function));
        chain.pop();
        assert_eq!(chain.depth(), 0);
    }

    #[test]
    fn current_returns_innermost_scope() {
        let mut chain = ScopeChain::new();
        chain.push(make_scope("outer", "proj.outer", NodeLabel::Module));
        chain.push(make_scope("inner", "proj.outer.inner", NodeLabel::Function));
        let current = chain.current().unwrap();
        assert_eq!(current.name, "inner");
        assert_eq!(current.qn, "proj.outer.inner");
    }

    #[test]
    fn current_qn_returns_innermost_qn() {
        let mut chain = ScopeChain::new();
        chain.push(make_scope("a", "proj.a", NodeLabel::Module));
        chain.push(make_scope("b", "proj.a.b", NodeLabel::Function));
        assert_eq!(chain.current_qn(), Some("proj.a.b"));
    }

    #[test]
    fn push_multiple_scopes() {
        let mut chain = ScopeChain::new();
        chain.push(make_scope("a", "proj.a", NodeLabel::Module));
        chain.push(make_scope("b", "proj.a.b", NodeLabel::Class));
        chain.push(make_scope("c", "proj.a.b.c", NodeLabel::Function));
        assert_eq!(chain.depth(), 3);
        assert_eq!(chain.current().unwrap().name, "c");
    }

    #[test]
    fn pop_then_push_works() {
        let mut chain = ScopeChain::new();
        chain.push(make_scope("a", "proj.a", NodeLabel::Module));
        chain.pop();
        assert!(chain.is_empty());
        chain.push(make_scope("b", "proj.b", NodeLabel::Function));
        assert_eq!(chain.depth(), 1);
        assert_eq!(chain.current().unwrap().name, "b");
    }

    // --- resolve_name ---

    #[test]
    fn resolve_name_finds_in_innermost_scope() {
        let mut chain = ScopeChain::new();
        chain.push(make_scope("foo", "proj.foo", NodeLabel::Function));
        assert_eq!(chain.resolve_name("foo").as_deref(), Some("proj.foo"));
    }

    #[test]
    fn resolve_name_finds_in_outer_scope() {
        let mut chain = ScopeChain::new();
        chain.push(make_scope("outer", "proj.outer", NodeLabel::Module));
        chain.push(make_scope("inner", "proj.outer.inner", NodeLabel::Function));
        // "outer" is not in the inner scope, but should be found in the outer.
        assert_eq!(chain.resolve_name("outer").as_deref(), Some("proj.outer"));
    }

    #[test]
    fn resolve_name_prefers_innermost_match() {
        let mut chain = ScopeChain::new();
        chain.push(make_scope("x", "proj.outer.x", NodeLabel::Function));
        chain.push(make_scope("x", "proj.outer.inner.x", NodeLabel::Function));
        // Both scopes have "x"; the innermost should win.
        assert_eq!(
            chain.resolve_name("x").as_deref(),
            Some("proj.outer.inner.x")
        );
    }

    #[test]
    fn resolve_name_returns_none_if_not_found() {
        let mut chain = ScopeChain::new();
        chain.push(make_scope("foo", "proj.foo", NodeLabel::Function));
        assert!(chain.resolve_name("bar").is_none());
    }

    #[test]
    fn resolve_name_searches_all_scopes() {
        let mut chain = ScopeChain::new();
        chain.push(make_scope("a", "proj.a", NodeLabel::Module));
        chain.push(make_scope("b", "proj.a.b", NodeLabel::Class));
        chain.push(make_scope("c", "proj.a.b.c", NodeLabel::Function));
        // "a" is in the outermost scope; should be found.
        assert_eq!(chain.resolve_name("a").as_deref(), Some("proj.a"));
        // "b" is in the middle scope; should be found.
        assert_eq!(chain.resolve_name("b").as_deref(), Some("proj.a.b"));
        // "c" is in the innermost scope; should be found.
        assert_eq!(chain.resolve_name("c").as_deref(), Some("proj.a.b.c"));
    }

    // --- iter ---

    #[test]
    fn iter_traverses_outermost_to_innermost() {
        let mut chain = ScopeChain::new();
        chain.push(make_scope("a", "proj.a", NodeLabel::Module));
        chain.push(make_scope("b", "proj.a.b", NodeLabel::Function));
        chain.push(make_scope("c", "proj.a.b.c", NodeLabel::Function));

        let names: Vec<&str> = chain.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn iter_on_empty_chain_yields_nothing() {
        let chain = ScopeChain::new();
        assert_eq!(chain.iter().count(), 0);
    }

    #[test]
    fn iter_count_matches_depth() {
        let mut chain = ScopeChain::new();
        chain.push(make_scope("a", "proj.a", NodeLabel::Module));
        chain.push(make_scope("b", "proj.a.b", NodeLabel::Function));
        assert_eq!(chain.iter().count(), chain.depth());
    }

    // --- Scope struct ---

    #[test]
    fn scope_new_creates_without_parent() {
        let scope = Scope::new("foo", "proj.foo", NodeLabel::Function);
        assert_eq!(scope.name, "foo");
        assert_eq!(scope.qn, "proj.foo");
        assert_eq!(scope.label, NodeLabel::Function);
        assert!(scope.parent.is_none());
    }

    #[test]
    fn scope_with_parent_sets_parent() {
        let scope = Scope::new("foo", "proj.foo", NodeLabel::Function).with_parent("proj");
        assert_eq!(scope.parent.as_deref(), Some("proj"));
    }

    #[test]
    fn scope_clone_is_equal() {
        let scope = Scope::new("foo", "proj.foo", NodeLabel::Function).with_parent("proj");
        let cloned = scope.clone();
        assert_eq!(scope.name, cloned.name);
        assert_eq!(scope.qn, cloned.qn);
        assert_eq!(scope.label, cloned.label);
        assert_eq!(scope.parent, cloned.parent);
    }

    #[test]
    fn scope_chain_default_is_empty() {
        let chain = ScopeChain::default();
        assert!(chain.is_empty());
    }

    #[test]
    fn scope_chain_clone_preserves_scopes() {
        let mut chain = ScopeChain::new();
        chain.push(make_scope("a", "proj.a", NodeLabel::Module));
        let cloned = chain.clone();
        assert_eq!(cloned.depth(), 1);
        assert_eq!(cloned.current().unwrap().name, "a");
    }

    #[test]
    fn scope_accepts_string_and_str() {
        let scope = Scope::new(
            String::from("foo"),
            String::from("proj.foo"),
            NodeLabel::Function,
        );
        assert_eq!(scope.name, "foo");
        assert_eq!(scope.qn, "proj.foo");
    }

    #[test]
    fn scope_with_parent_accepts_string_and_str() {
        let scope =
            Scope::new("foo", "proj.foo", NodeLabel::Function).with_parent(String::from("proj"));
        assert_eq!(scope.parent.as_deref(), Some("proj"));
    }

    #[test]
    fn debug_format_contains_name_and_qn() {
        let scope = Scope::new("foo", "proj.foo", NodeLabel::Function);
        let debug = format!("{scope:?}");
        assert!(debug.contains("foo"));
        assert!(debug.contains("proj.foo"));
    }
}

// ---------------------------------------------------------------------------
// ScopeResolver tests (Task 2.6)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod resolver_tests {
    use super::*;
    use crate::parse::parser_factory::ParserFactory;

    /// Parses `source` with the given language and returns the `Tree`.
    fn parse(language: Language, source: &str) -> Option<tree_sitter::Tree> {
        let mut parser = ParserFactory::create_parser(language).ok()?;
        parser.parse(source, None)
    }

    /// Collects all scopes from the root's named children using `resolver`.
    fn collect_scopes(
        resolver: &dyn ScopeResolver,
        root: tree_sitter::Node,
        ctx: &ScopeContext,
    ) -> Vec<Scope> {
        let mut scopes = Vec::new();
        for i in 0..root.named_child_count() as u32 {
            if let Some(child) = root.named_child(i) {
                if let Some(scope) = resolver.resolve(child, ctx) {
                    scopes.push(scope);
                }
            }
        }
        scopes
    }

    // --- ScopeResolverRegistry ---

    #[test]
    fn registry_new_is_not_empty() {
        let registry = ScopeResolverRegistry::new();
        assert!(!registry.is_empty());
        assert_eq!(registry.len(), Language::all().len());
    }

    #[test]
    fn registry_get_returns_resolver_for_compiled_in_languages() {
        let registry = ScopeResolverRegistry::new();
        for lang in Language::all() {
            assert!(
                registry.get(lang).is_some(),
                "missing resolver for {lang:?}"
            );
        }
    }

    #[test]
    fn registry_default_is_empty() {
        let registry = ScopeResolverRegistry::default();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    // --- PythonScopeResolver ---

    #[cfg(feature = "lang-python")]
    #[test]
    fn python_resolves_function_definition() {
        let source = String::from("def foo():\n    pass\n");
        let tree = parse(Language::Python, &source).expect("parse");
        let resolver = PythonScopeResolver;
        let ctx = ScopeContext {
            source: &source,
            file_path: "src/main.py",
            project: "proj",
            current_parent: None,
        };
        let scopes = collect_scopes(&resolver, tree.root_node(), &ctx);
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].name, "foo");
        assert_eq!(scopes[0].label, NodeLabel::Function);
        assert!(scopes[0].parent.is_none());
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn python_resolves_class_definition() {
        let source = String::from("class MyClass:\n    pass\n");
        let tree = parse(Language::Python, &source).expect("parse");
        let resolver = PythonScopeResolver;
        let ctx = ScopeContext {
            source: &source,
            file_path: "src/main.py",
            project: "proj",
            current_parent: None,
        };
        let scopes = collect_scopes(&resolver, tree.root_node(), &ctx);
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].name, "MyClass");
        assert_eq!(scopes[0].label, NodeLabel::Class);
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn python_method_label_when_parent_present() {
        let source = String::from("def foo():\n    pass\n");
        let tree = parse(Language::Python, &source).expect("parse");
        let resolver = PythonScopeResolver;
        let ctx = ScopeContext {
            source: &source,
            file_path: "src/main.py",
            project: "proj",
            current_parent: Some("MyClass"),
        };
        let scopes = collect_scopes(&resolver, tree.root_node(), &ctx);
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].label, NodeLabel::Method);
        assert_eq!(scopes[0].parent.as_deref(), Some("MyClass"));
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn python_returns_none_for_non_scope_node() {
        let source = String::from("x = 1\n");
        let tree = parse(Language::Python, &source).expect("parse");
        let resolver = PythonScopeResolver;
        let ctx = ScopeContext {
            source: &source,
            file_path: "src/main.py",
            project: "proj",
            current_parent: None,
        };
        let scopes = collect_scopes(&resolver, tree.root_node(), &ctx);
        assert!(scopes.is_empty());
    }

    // --- RustScopeResolver ---

    #[cfg(feature = "lang-rust")]
    #[test]
    fn rust_resolves_function_item() {
        let source = String::from("fn foo() {}\n");
        let tree = parse(Language::Rust, &source).expect("parse");
        let resolver = RustScopeResolver;
        let ctx = ScopeContext {
            source: &source,
            file_path: "src/main.rs",
            project: "proj",
            current_parent: None,
        };
        let scopes = collect_scopes(&resolver, tree.root_node(), &ctx);
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].name, "foo");
        assert_eq!(scopes[0].label, NodeLabel::Function);
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn rust_resolves_struct_item() {
        let source = String::from("struct Foo { x: i32 }\n");
        let tree = parse(Language::Rust, &source).expect("parse");
        let resolver = RustScopeResolver;
        let ctx = ScopeContext {
            source: &source,
            file_path: "src/main.rs",
            project: "proj",
            current_parent: None,
        };
        let scopes = collect_scopes(&resolver, tree.root_node(), &ctx);
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].name, "Foo");
        assert_eq!(scopes[0].label, NodeLabel::Struct);
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn rust_resolves_trait_and_impl() {
        let source = String::from("trait Foo { fn bar(); }\nimpl Foo for Bar {}\n");
        let tree = parse(Language::Rust, &source).expect("parse");
        let resolver = RustScopeResolver;
        let ctx = ScopeContext {
            source: &source,
            file_path: "src/main.rs",
            project: "proj",
            current_parent: None,
        };
        let scopes = collect_scopes(&resolver, tree.root_node(), &ctx);
        assert_eq!(scopes.len(), 2);
        assert_eq!(scopes[0].name, "Foo");
        assert_eq!(scopes[0].label, NodeLabel::Trait);
        assert_eq!(scopes[1].label, NodeLabel::Impl);
    }

    // --- CScopeResolver ---

    #[cfg(feature = "lang-c")]
    #[test]
    fn c_resolves_function_definition() {
        let source = String::from("int foo(void) { return 0; }\n");
        let tree = parse(Language::C, &source).expect("parse");
        let resolver = CScopeResolver;
        let ctx = ScopeContext {
            source: &source,
            file_path: "src/main.c",
            project: "proj",
            current_parent: None,
        };
        let scopes = collect_scopes(&resolver, tree.root_node(), &ctx);
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].name, "foo");
        assert_eq!(scopes[0].label, NodeLabel::Function);
    }

    #[cfg(feature = "lang-c")]
    #[test]
    fn c_resolves_struct_specifier() {
        let source = String::from("struct Foo { int x; };\n");
        let tree = parse(Language::C, &source).expect("parse");
        let resolver = CScopeResolver;
        let ctx = ScopeContext {
            source: &source,
            file_path: "src/main.c",
            project: "proj",
            current_parent: None,
        };
        let scopes = collect_scopes(&resolver, tree.root_node(), &ctx);
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].name, "Foo");
        assert_eq!(scopes[0].label, NodeLabel::Struct);
    }

    // --- FortranScopeResolver ---

    #[cfg(feature = "lang-fortran")]
    #[test]
    fn fortran_resolves_module() {
        let source = String::from("module mymod\ncontains\nend module\n");
        let tree = parse(Language::Fortran, &source).expect("parse");
        let resolver = FortranScopeResolver;
        let ctx = ScopeContext {
            source: &source,
            file_path: "src/mod.f90",
            project: "proj",
            current_parent: None,
        };
        let scopes = collect_scopes(&resolver, tree.root_node(), &ctx);
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].name, "mymod");
        assert_eq!(scopes[0].label, NodeLabel::Module);
    }

    #[cfg(feature = "lang-fortran")]
    #[test]
    fn fortran_resolves_subroutine() {
        let source = String::from("subroutine mysub(a)\n  integer :: a\nend subroutine\n");
        let tree = parse(Language::Fortran, &source).expect("parse");
        let resolver = FortranScopeResolver;
        let ctx = ScopeContext {
            source: &source,
            file_path: "src/mod.f90",
            project: "proj",
            current_parent: None,
        };
        let scopes = collect_scopes(&resolver, tree.root_node(), &ctx);
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].name, "mysub");
        assert_eq!(scopes[0].label, NodeLabel::Function);
    }

    // --- TypeScriptScopeResolver ---

    #[cfg(feature = "lang-typescript")]
    #[test]
    fn typescript_resolves_function_declaration() {
        let source = String::from("function foo(): void {}\n");
        let tree = parse(Language::TypeScript, &source).expect("parse");
        let resolver = TypeScriptScopeResolver;
        let ctx = ScopeContext {
            source: &source,
            file_path: "src/main.ts",
            project: "proj",
            current_parent: None,
        };
        let scopes = collect_scopes(&resolver, tree.root_node(), &ctx);
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].name, "foo");
        assert_eq!(scopes[0].label, NodeLabel::Function);
    }

    #[cfg(feature = "lang-typescript")]
    #[test]
    fn typescript_resolves_class_and_interface() {
        let source = String::from("class Foo {}\ninterface Bar {}\n");
        let tree = parse(Language::TypeScript, &source).expect("parse");
        let resolver = TypeScriptScopeResolver;
        let ctx = ScopeContext {
            source: &source,
            file_path: "src/main.ts",
            project: "proj",
            current_parent: None,
        };
        let scopes = collect_scopes(&resolver, tree.root_node(), &ctx);
        assert_eq!(scopes.len(), 2);
        assert_eq!(scopes[0].name, "Foo");
        assert_eq!(scopes[0].label, NodeLabel::Class);
        assert_eq!(scopes[1].name, "Bar");
        assert_eq!(scopes[1].label, NodeLabel::Interface);
    }

    // --- Registry dispatch ---

    #[test]
    fn registry_dispatches_by_language() {
        let registry = ScopeResolverRegistry::new();
        let source = String::new();
        let ctx = ScopeContext {
            source: &source,
            file_path: "",
            project: "",
            current_parent: None,
        };
        // Every compiled-in language should have a dispatchable resolver.
        for lang in Language::all() {
            let resolver = registry.get(lang).expect("resolver exists");
            // resolve on a root with no children returns None.
            let mut parser = ParserFactory::create_parser(lang).expect("parser");
            let tree = parser.parse(&source, None).expect("tree");
            let root = tree.root_node();
            assert!(resolver.resolve(root, &ctx).is_none());
        }
    }
}
