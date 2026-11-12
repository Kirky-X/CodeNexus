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
//! [`LspRegistryModule`] exposes the table as a kit capability
//! (`Arc<dyn LspRegistry>`); kit-less callers (the DB-free `lsp_*` CLI
//! commands, the indexing enrichment pass) construct the default registry
//! via [`default_registry`].

use std::path::Path;
use std::sync::Arc;

use super::LspProvider;
use crate::kit::{AsyncAutoBuilder, AsyncKit, ModuleMeta};

/// Registration bound for the default provider table.
///
/// [`start_active_providers_parallel`](crate::service::index) spawns one
/// thread per active provider without a semaphore, relying on the wired set
/// being small (≤8) and LSP startup being IO-bound. The default table asserts
/// against this const at construction so a 9th provider cannot slip in
/// silently.
pub const MAX_LSP_PROVIDERS: usize = 8;

/// Extension → provider lookup over the registered table.
pub trait LspRegistry: Send + Sync {
    /// Returns the provider registered for `file`'s extension, or `None`
    /// when no provider covers it (the caller surfaces a precise error —
    /// there is no implicit default backend).
    fn select(&self, file: &Path) -> Option<Arc<dyn LspProvider>>;

    /// All registered `(extension, provider)` pairs in registration order.
    /// The indexing enrichment pass uses the extensions to start only the
    /// providers a project actually needs (L7-6 on-demand startup).
    fn providers(&self) -> Vec<(&'static str, Arc<dyn LspProvider>)>;
}

/// The wired default registry: the 8 production backends
/// (rust-analyzer, pyright, clangd ×2 for c/cpp, gopls, tsserver, fortls,
/// jdtls).
pub struct DefaultLspRegistry {
    table: Vec<(&'static str, Arc<dyn LspProvider>)>,
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
    pub fn with_default_providers() -> Self {
        use crate::lsp::{
            ClangdClient, FortlsClient, GoplsClient, JdtlsClient, PyrightClient,
            RustAnalyzerClient, TypeScriptLanguageClient,
        };

        let mut registry = Self::new();
        let defaults: Vec<(&'static str, Arc<dyn LspProvider>)> = vec![
            ("rs", Arc::new(RustAnalyzerClient::new())),
            ("py", Arc::new(PyrightClient::new())),
            ("c", Arc::new(ClangdClient::new())),
            ("cpp", Arc::new(ClangdClient::new())),
            ("go", Arc::new(GoplsClient::new())),
            ("ts", Arc::new(TypeScriptLanguageClient::new())),
            ("f90", Arc::new(FortlsClient::new())),
            ("java", Arc::new(JdtlsClient::new())),
        ];
        for (ext, provider) in defaults {
            registry.register(ext, provider);
        }
        assert!(
            registry.table.len() <= MAX_LSP_PROVIDERS,
            "MAX_LSP_PROVIDERS={} but {} providers wired; raise the const or remove a provider",
            MAX_LSP_PROVIDERS,
            registry.table.len()
        );
        registry
    }

    /// Registers `provider` for `ext`. Re-registrations replace the existing
    /// entry for that extension (one provider per ext — the enrichment pass
    /// starts one server per active extension, and duplicated entries would
    /// spawn two), so the later registration wins `select`.
    pub fn register(&mut self, ext: &'static str, provider: Arc<dyn LspProvider>) {
        self.table.retain(|(registered, _)| *registered != ext);
        self.table.push((ext, provider));
    }
}

impl Default for DefaultLspRegistry {
    fn default() -> Self {
        Self::with_default_providers()
    }
}

impl LspRegistry for DefaultLspRegistry {
    fn select(&self, file: &Path) -> Option<Arc<dyn LspProvider>> {
        let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
        self.table
            .iter()
            .find(|(registered, _)| *registered == ext)
            .map(|(_, provider)| Arc::clone(provider))
    }

    fn providers(&self) -> Vec<(&'static str, Arc<dyn LspProvider>)> {
        self.table.clone()
    }
}

/// Constructs the wired default registry — the sanctioned path for callers
/// outside the kit (DB-free `lsp_*` commands, the indexing enrichment pass).
#[cfg(feature = "lsp")]
#[must_use]
pub fn default_registry() -> Arc<dyn LspRegistry> {
    Arc::new(DefaultLspRegistry::with_default_providers())
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
    type Error = super::LspError;

    fn build<'a>(
        _kit: &'a AsyncKit,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Capability, Self::Error>> + Send + 'a>,
    > {
        Box::pin(async move { Ok(default_registry()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::LspError;

    /// Minimal backend for registration tests — every LSP operation is a
    /// no-op; only identity/selection semantics are under test here.
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
    /// by extension — no match arm or constant anywhere else changes.
    #[test]
    fn select_hits_registered_provider_by_extension_without_match_edits() {
        let mut registry = DefaultLspRegistry::new();
        let provider: Arc<dyn LspProvider> = Arc::new(FakeProvider);
        registry.register("xyz", Arc::clone(&provider));

        let selected = registry.select(Path::new("/tmp/module.xyz")).expect("hit");
        assert!(
            Arc::ptr_eq(&selected, &provider),
            "select must return the registered instance"
        );
    }

    /// No implicit default backend: an uncovered extension selects `None`
    /// so the caller can produce a precise error.
    #[test]
    fn select_returns_none_for_unregistered_extension() {
        let mut registry = DefaultLspRegistry::new();
        registry.register("xyz", Arc::new(FakeProvider));
        assert!(registry.select(Path::new("/tmp/notes.txt")).is_none());
        assert!(registry.select(Path::new("/tmp/noext")).is_none());
    }

    /// The startup provider set is unchanged: 8 wired entries with the
    /// historical extension coverage.
    #[test]
    fn default_registry_wires_the_startup_provider_set() {
        let registry = DefaultLspRegistry::with_default_providers();
        let providers = registry.providers();
        assert_eq!(providers.len(), 8);
        let exts: Vec<&str> = providers.iter().map(|(ext, _)| *ext).collect();
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

    /// Duplicate registration: the later entry wins `select` (extension
    /// remapping without touching the table order).
    #[test]
    fn later_registration_wins_select_for_duplicated_extension() {
        let mut registry = DefaultLspRegistry::new();
        registry.register("rs", Arc::new(FakeProvider));
        let replacement: Arc<dyn LspProvider> = Arc::new(FakeProvider);
        registry.register("rs", Arc::clone(&replacement));

        let selected = registry.select(Path::new("/tmp/lib.rs")).expect("hit");
        assert!(Arc::ptr_eq(&selected, &replacement));
        assert_eq!(
            registry.providers().len(),
            1,
            "re-registration replaces, not appends (one provider per ext)"
        );
    }

    /// Kit capability: the module builds the wired default registry.
    #[tokio::test]
    async fn module_builds_default_registry_capability() {
        let kit = AsyncKit::new();
        let cap = LspRegistryModule::build(&kit).await.expect("build");
        assert_eq!(cap.providers().len(), 8);
        assert!(cap.select(Path::new("/tmp/lib.rs")).is_some());
    }
}
