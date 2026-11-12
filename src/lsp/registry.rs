// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! LSP provider registry — the single place where file extensions map to
//! [`LspProvider`] backends.
//!
//! Owns the provider table previously inlined in `service/index.rs`
//! (`build_lsp_providers`) and the per-extension match previously in
//! `service/lsp.rs` (`select_provider`). Adding a provider is a registration
//! change only: no match arm, no constant bump at any call site.
//!
//! # Factory semantics
//!
//! Entries carry a **factory**, not an instance: LSP clients hold mutable
//! session state (`Mutex<Option<Session>>`), so every [`LspRegistry::select`]
//! materializes a fresh client (matching the pre-registry behavior where
//! `select_provider` constructed one per call) and the caller owns it for
//! exactly one command cycle (`start` → query → `shutdown`).
//!
//! [`LspRegistryModule`] exposes the table as a kit capability
//! (`Arc<dyn LspRegistry>`); kit-less callers (the DB-free `lsp_*` CLI
//! commands, the indexing enrichment pass) construct the production registry
//! via [`DefaultLspRegistry::production`].

use std::path::Path;
use std::sync::Arc;

use super::LspProvider;
use crate::kit::{AsyncAutoBuilder, AsyncKit, ModuleMeta};

/// Registration bound for the production provider table.
///
/// [`start_active_providers_parallel`](crate::service::index) spawns one
/// thread per active provider without a semaphore, relying on the wired set
/// being small (≤8) and LSP startup being IO-bound. [`production()`]
/// asserts against this const so a 9th provider cannot slip in silently.
pub const MAX_LSP_PROVIDERS: usize = 8;

/// Materializes a fresh client instance — LSP clients are stateful (session
/// mutex), so sharing one instance across commands is unsafe.
pub type ProviderFactory = fn() -> Box<dyn LspProvider>;

/// One registry row: `extension` + the factory that materializes a fresh
/// backend instance for it.
#[derive(Clone, Copy)]
pub struct LspProviderEntry {
    /// File extension (without the leading dot) this row covers.
    pub extension: &'static str,
    /// Materializes a fresh client instance (see [`ProviderFactory`]).
    pub factory: ProviderFactory,
}

/// Extension → provider lookup over the registered table.
pub trait LspRegistry: Send + Sync {
    /// Materializes the provider registered for `file`'s extension, or
    /// `None` when no provider covers it (the caller surfaces a precise
    /// error — there is no implicit default backend).
    fn select(&self, file: &Path) -> Option<Arc<dyn LspProvider>>;

    /// All registered rows in registration order. The indexing enrichment
    /// pass uses the extensions to start only the providers a project
    /// actually needs (L7-6 on-demand startup).
    fn providers(&self) -> Vec<LspProviderEntry>;
}

/// Registry failures surfaced through the kit capability.
#[derive(Debug, thiserror::Error)]
pub enum LspRegistryError {
    /// The `LspRegistryModule` capability is not resolvable — the `lsp`
    /// feature is disabled or the module was not registered.
    #[error("LSP registry capability not available")]
    NotAvailable,
}

/// The wired production registry: the 8 production backends
/// (rust-analyzer, pyright, clangd ×2 for c/cpp, gopls, tsserver, fortls,
/// jdtls).
pub struct DefaultLspRegistry {
    table: Vec<LspProviderEntry>,
}

impl DefaultLspRegistry {
    /// An empty registry (tests, incremental registration).
    #[must_use]
    pub fn new() -> Self {
        Self { table: Vec::new() }
    }

    /// The production table — the exact set `service/index.rs` wired before
    /// the registry existed.
    ///
    /// # Panics
    ///
    /// Asserts the wired set stays within [`MAX_LSP_PROVIDERS`] (the
    /// semaphore-free startup bound).
    #[must_use]
    pub fn production() -> Self {
        let mut registry = Self::new();
        let defaults: [(&'static str, ProviderFactory); 8] = [
            ("rs", || Box::new(crate::lsp::RustAnalyzerClient::new())),
            ("py", || Box::new(crate::lsp::PyrightClient::new())),
            ("c", || Box::new(crate::lsp::ClangdClient::new())),
            ("cpp", || Box::new(crate::lsp::ClangdClient::new())),
            ("go", || Box::new(crate::lsp::GoplsClient::new())),
            ("ts", || {
                Box::new(crate::lsp::TypeScriptLanguageClient::new())
            }),
            ("f90", || Box::new(crate::lsp::FortlsClient::new())),
            ("java", || Box::new(crate::lsp::JdtlsClient::new())),
        ];
        for (ext, factory) in defaults {
            registry.register(ext, factory);
        }
        assert!(
            registry.table.len() <= MAX_LSP_PROVIDERS,
            "MAX_LSP_PROVIDERS={} but {} providers wired; raise the const or remove a provider",
            MAX_LSP_PROVIDERS,
            registry.table.len()
        );
        registry
    }

    /// Registers `factory` for `ext`. Re-registration replaces the existing
    /// row for that extension (one provider per ext — the enrichment pass
    /// starts one server per active extension, and duplicated rows would
    /// spawn two), so the later registration wins `select`.
    pub fn register(&mut self, ext: &'static str, factory: ProviderFactory) {
        self.table.retain(|entry| entry.extension != ext);
        self.table.push(LspProviderEntry {
            extension: ext,
            factory,
        });
    }
}

impl Default for DefaultLspRegistry {
    fn default() -> Self {
        Self::production()
    }
}

impl LspRegistry for DefaultLspRegistry {
    fn select(&self, file: &Path) -> Option<Arc<dyn LspProvider>> {
        let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
        self.table
            .iter()
            .find(|entry| entry.extension == ext)
            .map(|entry| Arc::from((entry.factory)()))
    }

    fn providers(&self) -> Vec<LspProviderEntry> {
        self.table.clone()
    }
}

/// Constructs the wired production registry — the sanctioned path for
/// callers outside the kit (DB-free `lsp_*` commands, the indexing
/// enrichment pass).
#[must_use]
pub fn default_registry() -> Arc<dyn LspRegistry> {
    Arc::new(DefaultLspRegistry::production())
}

/// Kit capability module wrapping [`DefaultLspRegistry`].
///
/// Registered in `build_kit` under the `lsp` feature; the built capability
/// is `Arc<dyn LspRegistry>`, so `Nexus`/`kit.require` consumers get the
/// registry without touching provider construction.
pub struct LspRegistryModule;

impl ModuleMeta for LspRegistryModule {
    const NAME: &'static str = "lsp_registry";
    fn dependencies() -> &'static [(&'static str, std::any::TypeId)] {
        &[]
    }
}

impl AsyncAutoBuilder for LspRegistryModule {
    type Capability = Arc<dyn LspRegistry>;
    type Error = LspRegistryError;

    fn build<'a>(
        _kit: &'a AsyncKit,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Capability, Self::Error>> + Send + 'a>,
    > {
        Box::pin(
            async move { Ok(Arc::new(DefaultLspRegistry::production()) as Arc<dyn LspRegistry>) },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::LspError;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Construction counter for [`fake_factory`] — registry rows hold `fn`
    /// pointers (non-capturing), so the counter lives in a static.
    static FAKE_CONSTRUCTIONS: AtomicUsize = AtomicUsize::new(0);

    /// The factory registered in tests: counts one construction per call.
    fn fake_factory() -> Box<dyn LspProvider> {
        FAKE_CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst);
        Box::new(FakeProvider)
    }

    /// Minimal backend for registration tests — every LSP operation is a
    /// no-op.
    struct FakeProvider;
    impl LspProvider for FakeProvider {
        fn start(&self, _workspace: &Path) -> Result<(), LspError> {
            Ok(())
        }
        fn definition(
            &self,
            _file: &Path,
            _line: u32,
            _col: u32,
        ) -> Result<Option<lsp_types::Location>, LspError> {
            Ok(None)
        }
        fn type_definition(
            &self,
            _file: &Path,
            _line: u32,
            _col: u32,
        ) -> Result<Option<lsp_types::Location>, LspError> {
            Ok(None)
        }
        fn hover(
            &self,
            _file: &Path,
            _line: u32,
            _col: u32,
        ) -> Result<Option<lsp_types::Hover>, LspError> {
            Ok(None)
        }
        fn shutdown(&self) -> Result<(), LspError> {
            Ok(())
        }
    }

    /// R-lsp-001 acceptance: a newly registered provider is selected purely
    /// by extension — no match arm or constant anywhere else changes — and
    /// the selection materializes through the registered factory.
    #[test]
    fn select_hits_registered_provider_by_extension_without_match_edits() {
        // Other tests share this static counter and run concurrently, so
        // assert on the DELTA around the call, not an absolute value.
        let before = FAKE_CONSTRUCTIONS.load(Ordering::SeqCst);
        let mut registry = DefaultLspRegistry::new();
        registry.register("xyz", fake_factory);

        let selected = registry.select(Path::new("/tmp/module.xyz")).expect("hit");
        let _ = selected.shutdown();
        assert_eq!(
            FAKE_CONSTRUCTIONS.load(Ordering::SeqCst),
            before + 1,
            "select must materialize through the registered factory"
        );
    }

    /// No implicit default backend: an uncovered extension selects `None`
    /// so the caller can produce a precise error.
    #[test]
    fn select_returns_none_for_unregistered_extension() {
        let mut registry = DefaultLspRegistry::new();
        registry.register("xyz", fake_factory);
        assert!(registry.select(Path::new("/tmp/notes.txt")).is_none());
        assert!(registry.select(Path::new("/tmp/noext")).is_none());
    }

    /// The startup provider set is unchanged: 8 wired rows with the
    /// historical extension coverage.
    #[test]
    fn production_wires_the_startup_provider_set() {
        let registry = DefaultLspRegistry::production();
        let providers = registry.providers();
        assert_eq!(providers.len(), 8);
        let exts: Vec<&str> = providers.iter().map(|entry| entry.extension).collect();
        assert_eq!(
            exts,
            vec!["rs", "py", "c", "cpp", "go", "ts", "f90", "java"]
        );
        for file in [
            "/tmp/lib.rs",
            "/tmp/m.py",
            "/tmp/a.c",
            "/tmp/a.cpp",
            "/tmp/m.go",
            "/tmp/m.ts",
            "/tmp/m.f90",
            "/tmp/M.java",
        ] {
            assert!(
                registry.select(Path::new(file)).is_some(),
                "no provider selected for {file}"
            );
        }
    }

    /// Factory semantics: two selections of the same extension return
    /// DISTINCT instances — LSP clients are stateful and must not be shared
    /// across command cycles.
    #[test]
    fn select_materializes_a_fresh_instance_per_call() {
        let registry = DefaultLspRegistry::production();
        let first = registry.select(Path::new("/tmp/lib.rs")).expect("hit");
        let second = registry.select(Path::new("/tmp/other.rs")).expect("hit");
        assert!(
            !Arc::ptr_eq(&first, &second),
            "select must not hand out a shared sessionful instance"
        );
    }

    /// Re-registration replaces the row for that extension (one provider
    /// per ext — duplicated rows would spawn two servers per extension in
    /// the enrichment pass).
    #[test]
    fn later_registration_replaces_duplicated_extension() {
        let mut registry = DefaultLspRegistry::new();
        registry.register("rs", fake_factory);
        registry.register("rs", fake_factory);
        assert_eq!(
            registry.providers().len(),
            1,
            "re-registration replaces, not appends (one provider per ext)"
        );
        assert!(registry.select(Path::new("/tmp/lib.rs")).is_some());
    }

    /// Kit capability: the module builds the wired production registry.
    #[tokio::test]
    async fn module_builds_default_registry_capability() {
        let kit = AsyncKit::new();
        let cap = LspRegistryModule::build(&kit).await.expect("build");
        assert_eq!(cap.providers().len(), 8);
        assert!(cap.select(Path::new("/tmp/lib.rs")).is_some());
    }
}
