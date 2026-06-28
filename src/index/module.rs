// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! trait-kit module for the Indexer subsystem (T6/unified-architecture
//! Phase 2, Task 2.7).
//!
//! Implements [`Module`] / [`ModuleBuilder`] / [`WithConfig`] for
//! [`IndexerModule`], wiring the existing [`IndexFacade`] (Facade pattern)
//! into the unified Kit registry as `Arc<dyn Indexer>` under
//! [`IndexerKey`](crate::kit::IndexerKey).
//!
//! # Dependency note
//!
//! Conceptually the Indexer depends on `StorageKey` (it persists the graph)
//! and `ExtractorKey` (it parses source files). The concrete [`IndexFacade`]
//! is self-contained, however: it stores only the `db_path` and opens a fresh
//! [`Repository`](crate::storage::Repository) per `index*` call, and
//! [`parallel_parse`](crate::parse::parallel_parse) uses the thread-local
//! [`ParserPool`](crate::parse::ParserPool) + [`get_extractor`](crate::parse::get_extractor)
//! directly. Therefore `Requirements = NoRequirements` at the type level; the
//! bootstrap (Task 2.13) enforces build ordering (Storage → Parser → Extractor
//! → Indexer).
//!
//! [`Module`]: crate::kit::Module
//! [`ModuleBuilder`]: crate::kit::ModuleBuilder
//! [`WithConfig`]: crate::kit::WithConfig

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::kit::{Module, ModuleBuilder, NoRequirements, WithConfig};

use super::capability::Indexer;
use super::error::IndexError;
use super::pipeline::{IndexFacade, IndexResult};
use crate::storage::StorageError;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for [`IndexerModule`] (Task 2.7).
///
/// Stored in Kit under [`IndexConfigKey`](crate::kit::IndexConfigKey) and
/// injected into [`IndexerModuleBuilder`] via [`WithConfig`]. The Indexer
/// needs only the database path — `force` is a per-call argument to
/// [`Indexer::index`] and `embed` is not yet wired (Phase 4, H10).
#[derive(Debug, Clone)]
pub struct IndexConfig {
    /// Filesystem path to the LadybugDB database directory.
    ///
    /// Pass `":memory:"` for an in-memory database (useful for tests).
    pub db_path: PathBuf,
}

impl IndexConfig {
    /// Creates a config pointing at an in-memory database.
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            db_path: PathBuf::from(":memory:"),
        }
    }
}

// ---------------------------------------------------------------------------
// Module + Builder
// ---------------------------------------------------------------------------

/// trait-kit module tag for the Indexer subsystem (Task 2.7).
///
/// Zero-sized marker — construction logic lives in
/// [`IndexerModuleBuilder::build`]. Register in Kit via:
///
/// ```ignore
/// use codenexus::kit::{IntoKitModuleBuilder, Kit, IndexerKey};
/// use codenexus::index::{IndexConfig, IndexerModuleBuilder};
///
/// let kit = Kit::new();
/// let indexer = IndexerModuleBuilder::new()
///     .config(IndexConfig::in_memory())
///     .kit(&kit)
///     .provide::<IndexerKey>()?;
/// ```
pub struct IndexerModule;

/// Builder for [`IndexerModule`] (Task 2.7).
///
/// Construct with [`IndexerModuleBuilder::new`], inject config with
/// [`WithConfig::config`], then attach to a [`Kit`](crate::kit::Kit) via
/// [`IntoKitModuleBuilder::kit`](crate::kit::IntoKitModuleBuilder::kit) and
/// call [`provide`](crate::kit::KitModuleBuilder::provide).
pub struct IndexerModuleBuilder {
    config: Option<IndexConfig>,
}

impl IndexerModuleBuilder {
    /// Creates a builder with no config set. Call `.config(...)` before
    /// building.
    #[must_use]
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for IndexerModuleBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for IndexerModule {
    type Config = IndexConfig;
    type Requirements = NoRequirements;
    type Capability = Arc<dyn Indexer>;
    type Error = IndexError;
    type Builder = IndexerModuleBuilder;
    const NAME: &'static str = "indexer";
}

impl ModuleBuilder<IndexerModule> for IndexerModuleBuilder {
    fn build(self) -> Result<Arc<dyn Indexer>, IndexError> {
        let config = self.config.ok_or_else(|| {
            IndexError::Storage(StorageError::InvalidData(
                "IndexerModuleBuilder requires config — call .config(IndexConfig { db_path }) before build"
                    .to_string(),
            ))
        })?;
        // IndexFacade stores the db_path and opens a Repository lazily on each
        // index* call, so the capability is cheap to construct.
        let facade = IndexFacade::new(&config.db_path)?;
        Ok(Arc::new(IndexerCapability { facade }))
    }
}

impl WithConfig<IndexerModule> for IndexerModuleBuilder {
    fn config(self, config: IndexConfig) -> Self {
        Self {
            config: Some(config),
        }
    }
}

// ---------------------------------------------------------------------------
// Concrete dyn Indexer implementation
// ---------------------------------------------------------------------------

/// Concrete implementation of [`dyn Indexer`] delegating to [`IndexFacade`].
///
/// [`IndexFacade`] holds only a [`PathBuf`], so it is `Send + Sync` and no
/// interior mutability is required (unlike [`StorageCapability`](crate::storage::module::StorageCapability)).
struct IndexerCapability {
    facade: IndexFacade,
}

impl Indexer for IndexerCapability {
    fn index(
        &self,
        path: &Path,
        project_name: &str,
        force: bool,
    ) -> Result<IndexResult, IndexError> {
        self.facade.index(path, project_name, force)
    }

    fn index_incremental(
        &self,
        path: &Path,
        project_name: &str,
        force: bool,
    ) -> Result<IndexResult, IndexError> {
        self.facade.index_incremental(path, project_name, force)
    }

    fn index_ram_first(
        &self,
        path: &Path,
        project_name: &str,
        force: bool,
    ) -> Result<IndexResult, IndexError> {
        self.facade.index_ram_first(path, project_name, force)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::IndexerKey;
    use std::fs;
    use tempfile::TempDir;

    /// Writes a file at `dir/rel` (creating parent directories as needed).
    fn write_file(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn builder_requires_config() {
        let result = IndexerModuleBuilder::new().build();
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(
            err.to_string().contains("config"),
            "missing-config error should mention config: {err}"
        );
    }

    #[test]
    fn build_returns_send_sync_capability() {
        let cap = IndexerModuleBuilder::new()
            .config(IndexConfig::in_memory())
            .build()
            .expect("build");
        // If this compiles, IndexerCapability is Send + Sync (the dyn Indexer
        // bound requires it). The Arc<dyn Indexer> is also Send + Sync.
        fn _assert_send_sync<T: Send + Sync>(_: &T) {}
        _assert_send_sync(&cap);
    }

    #[test]
    fn capability_index_empty_dir_returns_zero_files() {
        let cap = IndexerModuleBuilder::new()
            .config(IndexConfig::in_memory())
            .build()
            .expect("build");
        let tmp = TempDir::new().unwrap();
        let result = cap.index(tmp.path(), "empty", false).expect("index");
        assert_eq!(result.files_indexed, 0, "empty dir → 0 files indexed");
        assert_eq!(result.files_skipped, 0);
        assert!(result.duration_ms < u64::MAX, "duration should be recorded");
    }

    #[test]
    fn capability_index_real_files_creates_nodes() {
        let cap = IndexerModuleBuilder::new()
            .config(IndexConfig::in_memory())
            .build()
            .expect("build");
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "main.rs",
            "fn main() { helper(); }\nfn helper() {}\n",
        );

        let result = cap.index(tmp.path(), "demo", false).expect("index");
        assert!(
            result.files_indexed > 0,
            "should index files: {result:?}"
        );
        assert!(
            result.nodes_created > 0,
            "should create nodes: {result:?}"
        );
        assert!(!result.project_id.is_empty(), "project_id should be set");
    }

    #[test]
    fn capability_path_not_found_returns_error() {
        let cap = IndexerModuleBuilder::new()
            .config(IndexConfig::in_memory())
            .build()
            .expect("build");
        let result = cap.index(Path::new("/nonexistent/path/xyz"), "demo", false);
        assert!(result.is_err(), "path not found should error");
        let err = result.unwrap_err();
        assert!(
            matches!(err, IndexError::PathNotFound(_)),
            "expected PathNotFound, got {err:?}"
        );
        assert_eq!(
            err.exit_code(),
            1,
            "PRD §4.1.6: path not found → exit 1"
        );
    }

    /// Verify the full Kit registration flow works end-to-end.
    #[test]
    fn kit_registration_flow() {
        use crate::kit::{IntoKitModuleBuilder, Kit};

        let kit = Kit::new();
        let indexer = IndexerModuleBuilder::new()
            .config(IndexConfig::in_memory())
            .kit(&kit)
            .provide::<IndexerKey>()
            .expect("provide::<IndexerKey>");

        assert!(kit.contains::<IndexerKey>());

        let required = kit
            .require::<IndexerKey>()
            .expect("require::<IndexerKey>");
        assert!(Arc::ptr_eq(&indexer, &required));
    }
}
