//! Thread-local pool of tree-sitter parsers (ADR-010).
//!
//! [`ParserPool`] caches one [`Parser`] per [`Language`] to avoid the overhead
//! of repeated `Parser::new()` + `set_language()` calls during parallel
//! parsing. Each thread should use its own pool instance (or the thread-local
//! instance via [`with_thread_pool`]).

use std::cell::{RefCell, RefMut};
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};

use tree_sitter::Parser;

use crate::model::Language;

use super::error::Result;
use super::parser_factory::ParserFactory;

/// A pool of tree-sitter parsers that caches one [`Parser`] per [`Language`]
/// (ADR-010).
///
/// The pool uses interior mutability (`RefCell`) so parsers can be retrieved
/// via a shared reference. Each thread should use its own `ParserPool`
/// instance, or the thread-local instance accessed via [`with_thread_pool`].
///
/// # Example
///
/// ```
/// use codenexus::model::Language;
/// use codenexus::parse::ParserPool;
///
/// let pool = ParserPool::new();
/// let mut parser = pool.get_parser(Language::Rust).unwrap();
/// let tree = parser.parse("fn main() {}", None);
/// assert!(tree.is_some());
/// ```
pub struct ParserPool {
    parsers: RefCell<HashMap<Language, Parser>>,
}

impl ParserPool {
    /// Creates a new empty parser pool.
    #[must_use]
    pub fn new() -> Self {
        Self {
            parsers: RefCell::new(HashMap::new()),
        }
    }

    /// Returns a guard providing mutable access to a parser configured for the
    /// given language. The parser is created and cached on first access;
    /// subsequent calls for the same language reuse the cached parser.
    ///
    /// The returned [`ParserGuard`] implements `DerefMut<Target = Parser>`,
    /// so it can be used like `&mut Parser`.
    pub fn get_parser(&self, lang: Language) -> Result<ParserGuard<'_>> {
        let mut map = self.parsers.borrow_mut();
        if let std::collections::hash_map::Entry::Vacant(e) = map.entry(lang) {
            let parser = ParserFactory::create_parser(lang)?;
            e.insert(parser);
        }
        Ok(ParserGuard {
            inner: RefMut::map(map, |m| {
                m.get_mut(&lang).expect("parser was just inserted")
            }),
        })
    }

    /// Returns the number of cached parsers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.parsers.borrow().len()
    }

    /// Returns `true` if no parsers are cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.parsers.borrow().is_empty()
    }
}

impl Default for ParserPool {
    fn default() -> Self {
        Self::new()
    }
}

/// A guard providing mutable access to a pooled [`Parser`].
///
/// Implements [`Deref`] / [`DerefMut`] to `Parser`, so it can be used like
/// `&mut Parser`. The parser remains in the pool when the guard is dropped.
pub struct ParserGuard<'a> {
    inner: RefMut<'a, Parser>,
}

impl Deref for ParserGuard<'_> {
    type Target = Parser;

    fn deref(&self) -> &Parser {
        &self.inner
    }
}

impl DerefMut for ParserGuard<'_> {
    fn deref_mut(&mut self) -> &mut Parser {
        &mut self.inner
    }
}

// ---------------------------------------------------------------------------
// Thread-local pool (ADR-010): each thread gets its own ParserPool, avoiding
// synchronization overhead during rayon-based parallel parsing.
// ---------------------------------------------------------------------------

thread_local! {
    static THREAD_POOL: ParserPool = ParserPool::new();
}

/// Runs a closure with the thread-local [`ParserPool`].
///
/// Each thread gets its own pool instance, so there is no cross-thread
/// synchronization (ADR-010). The pool persists across calls on the same
/// thread, caching parsers for reuse.
///
/// # Example
///
/// ```
/// use codenexus::model::Language;
/// use codenexus::parse::with_thread_pool;
///
/// with_thread_pool(|pool| {
///     let mut parser = pool.get_parser(Language::Rust).unwrap();
///     let tree = parser.parse("fn main() {}", None);
///     assert!(tree.is_some());
/// });
/// ```
pub fn with_thread_pool<R>(f: impl FnOnce(&ParserPool) -> R) -> R {
    THREAD_POOL.with(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_pool_is_empty() {
        let pool = ParserPool::new();
        assert!(pool.is_empty());
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn default_creates_empty_pool() {
        let pool = ParserPool::default();
        assert!(pool.is_empty());
    }

    #[test]
    fn get_parser_creates_and_caches() {
        let pool = ParserPool::new();
        assert!(pool.is_empty());

        // First call creates the parser.
        {
            let parser = pool.get_parser(Language::Rust);
            assert!(parser.is_ok(), "get_parser should succeed for Rust");
        }
        assert_eq!(pool.len(), 1, "pool should have 1 parser after first call");

        // Second call reuses the cached parser.
        {
            let parser = pool.get_parser(Language::Rust);
            assert!(parser.is_ok());
        }
        assert_eq!(
            pool.len(),
            1,
            "pool should still have 1 parser (reused) after second call"
        );
    }

    #[test]
    fn get_parser_caches_multiple_languages() {
        let pool = ParserPool::new();
        for lang in Language::all() {
            let parser = pool.get_parser(lang);
            assert!(parser.is_ok(), "get_parser should succeed for {lang}");
        }
        assert_eq!(
            pool.len(),
            Language::all().len(),
            "pool should have one parser per language"
        );
    }

    #[test]
    fn get_parser_parses_rust() {
        let pool = ParserPool::new();
        let mut parser = pool.get_parser(Language::Rust).unwrap();
        let tree = parser.parse("fn main() {}", None);
        assert!(tree.is_some());
        assert!(!tree.unwrap().root_node().has_error());
    }

    #[test]
    fn get_parser_parses_c() {
        let pool = ParserPool::new();
        let mut parser = pool.get_parser(Language::C).unwrap();
        let tree = parser.parse("int main() { return 0; }", None);
        assert!(tree.is_some());
        assert!(!tree.unwrap().root_node().has_error());
    }

    #[test]
    fn get_parser_parses_python() {
        let pool = ParserPool::new();
        let mut parser = pool.get_parser(Language::Python).unwrap();
        let tree = parser.parse("def foo(): pass", None);
        assert!(tree.is_some());
        assert!(!tree.unwrap().root_node().has_error());
    }

    #[test]
    fn get_parser_parses_fortran() {
        let pool = ParserPool::new();
        let mut parser = pool.get_parser(Language::Fortran).unwrap();
        let tree = parser
            .parse("subroutine foo()\nend subroutine", None)
            .unwrap();
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn get_parser_parses_typescript() {
        let pool = ParserPool::new();
        let mut parser = pool.get_parser(Language::TypeScript).unwrap();
        let tree = parser
            .parse("function foo(): void {}", None)
            .unwrap();
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn parser_guard_derefs_to_parser() {
        let pool = ParserPool::new();
        let guard = pool.get_parser(Language::Rust).unwrap();
        // Deref<Target = Parser> allows calling &Parser methods.
        let _lang_ref = guard.language();
    }

    #[test]
    fn parser_guard_deref_mut_allows_parsing() {
        let pool = ParserPool::new();
        let mut guard = pool.get_parser(Language::Rust).unwrap();
        // DerefMut allows calling &mut Parser methods like parse().
        let tree = guard.parse("fn main() {}", None);
        assert!(tree.is_some());
    }

    #[test]
    fn pool_works_across_threads_with_scope() {
        // Each thread creates its own pool — no Sync required.
        std::thread::scope(|s| {
            s.spawn(|| {
                let pool = ParserPool::new();
                let mut parser = pool.get_parser(Language::Rust).unwrap();
                let tree = parser.parse("fn main() {}", None);
                assert!(tree.is_some(), "Rust parse should work on worker thread");
            });
            s.spawn(|| {
                let pool = ParserPool::new();
                let mut parser = pool.get_parser(Language::C).unwrap();
                let tree = parser.parse("int main() { return 0; }", None);
                assert!(tree.is_some(), "C parse should work on worker thread");
            });
        });
    }

    #[test]
    fn thread_local_pool_works() {
        with_thread_pool(|pool| {
            let mut parser = pool.get_parser(Language::Rust).unwrap();
            let tree = parser.parse("fn main() {}", None);
            assert!(tree.is_some());
        });
    }

    #[test]
    fn thread_local_pool_caches_across_calls() {
        with_thread_pool(|pool| {
            assert!(pool.is_empty());
            let _ = pool.get_parser(Language::Rust).unwrap();
            assert_eq!(pool.len(), 1);
        });
        // Second call on the same thread should reuse the cached parser.
        with_thread_pool(|pool| {
            assert_eq!(
                pool.len(),
                1,
                "thread-local pool should persist across calls on the same thread"
            );
            let _ = pool.get_parser(Language::C).unwrap();
            assert_eq!(pool.len(), 2);
        });
    }

    #[test]
    fn thread_local_pool_is_per_thread() {
        std::thread::scope(|s| {
            s.spawn(|| {
                with_thread_pool(|pool| {
                    assert!(pool.is_empty(), "new thread should have empty pool");
                    let _ = pool.get_parser(Language::Rust).unwrap();
                    assert_eq!(pool.len(), 1);
                });
            });
            s.spawn(|| {
                with_thread_pool(|pool| {
                    assert!(
                        pool.is_empty(),
                        "different thread should have its own empty pool"
                    );
                    let _ = pool.get_parser(Language::Python).unwrap();
                    assert_eq!(pool.len(), 1);
                });
            });
        });
    }

    #[test]
    fn pool_reuses_parser_on_same_thread() {
        // Verify that calling get_parser twice on the same thread reuses
        // the cached parser (pool size stays at 1).
        let pool = ParserPool::new();
        {
            let mut p1 = pool.get_parser(Language::Rust).unwrap();
            let tree = p1.parse("fn a() {}", None);
            assert!(tree.is_some());
        }
        {
            let mut p2 = pool.get_parser(Language::Rust).unwrap();
            let tree = p2.parse("fn b() {}", None);
            assert!(tree.is_some());
        }
        assert_eq!(pool.len(), 1, "parser should be reused, not recreated");
    }
}
