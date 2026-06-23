//! Scope chain for nested scope resolution (resolve/scope.rs).
//!
//! A scope chain represents the nesting of scopes
//! (file -> module -> class -> function -> block). Name resolution searches
//! from the innermost scope outward.

use crate::model::NodeLabel;

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
        let scope = Scope::new(String::from("foo"), String::from("proj.foo"), NodeLabel::Function);
        assert_eq!(scope.name, "foo");
        assert_eq!(scope.qn, "proj.foo");
    }

    #[test]
    fn scope_with_parent_accepts_string_and_str() {
        let scope = Scope::new("foo", "proj.foo", NodeLabel::Function)
            .with_parent(String::from("proj"));
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
