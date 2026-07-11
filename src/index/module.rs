// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! trait-kit module for the Indexer subsystem (T6/unified-architecture
//! Phase 2, Task 2.7; v0.3.3 AsyncKit migration).
//!
//! Implements [`ModuleMeta`] + [`AsyncAutoBuilder`] for [`IndexerModule`],
//! wiring the existing [`IndexFacade`] (Facade pattern) into the unified
//! Kit registry as `Arc<dyn Indexer>` under
//! [`IndexerModule`](crate::kit::IndexerModule).

use std::any::TypeId;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use crate::kit::{AsyncAutoBuilder, AsyncKit, ModuleMeta};

use super::capability::Indexer;
use super::error::IndexError;
use super::pipeline::{IndexFacade, IndexResult};
use crate::storage::StorageError;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for [`IndexerModule`] (Task 2.7).
///
/// Stored in Kit via `AsyncKit::set_config` and read in
/// [`AsyncAutoBuilder::build`].
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
// Module (ModuleMeta + AsyncAutoBuilder)
// ---------------------------------------------------------------------------

/// trait-kit module tag for the Indexer subsystem (Task 2.7).
///
/// Zero-sized marker — construction logic lives in
/// [`IndexerModule::build_cap`]. Register in Kit via:
///
/// ```ignore
/// use codenexus::kit::{AsyncKit, IndexerModule};
/// use codenexus::index::IndexConfig;
///
/// let mut kit = AsyncKit::new();
/// kit.set_config(IndexConfig::in_memory());
/// kit.register::<IndexerModule>()?;
/// let kit = kit.build().await?;
/// let indexer = kit.require::<IndexerModule>()?;
/// ```
pub struct IndexerModule;

impl ModuleMeta for IndexerModule {
    const NAME: &'static str = "indexer";
    fn dependencies() -> &'static [(&'static str, TypeId)] {
        &[]
    }
}

impl AsyncAutoBuilder for IndexerModule {
    type Capability = Arc<dyn Indexer>;
    type Error = IndexError;

    fn build<'a>(
        kit: &'a AsyncKit,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Capability, Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            let config = kit
                .config::<IndexConfig>()
                .map_err(|e| IndexError::Storage(StorageError::InvalidData(e.to_string())))?;
            Self::build_cap(&config)
        })
    }
}

impl IndexerModule {
    /// Constructs an IndexerCapability from the given config.
    ///
    /// Shared between [`AsyncAutoBuilder::build`] and tests.
    pub(crate) fn build_cap(config: &IndexConfig) -> Result<Arc<dyn Indexer>, IndexError> {
        let facade = IndexFacade::new(&config.db_path)?;
        Ok(Arc::new(IndexerCapability { facade }))
    }
}

// ---------------------------------------------------------------------------
// Concrete dyn Indexer implementation
// ---------------------------------------------------------------------------

/// Concrete implementation of [`dyn Indexer`] delegating to [`IndexFacade`].
///
/// [`IndexFacade`] holds only a [`PathBuf`], so it is `Send + Sync` and no
/// interior mutability is required.
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
    use crate::kit::{AsyncKit, IndexerModule};
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
    fn build_returns_send_sync_capability() {
        let cap = IndexerModule::build_cap(&IndexConfig::in_memory()).expect("build_cap");
        fn _assert_send_sync<T: Send + Sync>(_: &T) {}
        _assert_send_sync(&cap);
    }

    #[test]
    fn capability_index_empty_dir_returns_zero_files() {
        let cap = IndexerModule::build_cap(&IndexConfig::in_memory()).expect("build_cap");
        let tmp = TempDir::new().unwrap();
        let result = cap.index(tmp.path(), "empty", false).expect("index");
        assert_eq!(result.files_indexed, 0, "empty dir → 0 files indexed");
        assert_eq!(result.files_skipped, 0);
        assert!(result.duration_ms < u64::MAX, "duration should be recorded");
    }

    #[test]
    fn capability_index_real_files_creates_nodes() {
        let cap = IndexerModule::build_cap(&IndexConfig::in_memory()).expect("build_cap");
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "main.rs",
            "fn main() { helper(); }\nfn helper() {}\n",
        );

        let result = cap.index(tmp.path(), "demo", false).expect("index");
        assert!(result.files_indexed > 0, "should index files: {result:?}");
        assert!(result.nodes_created > 0, "should create nodes: {result:?}");
        assert!(!result.project_id.is_empty(), "project_id should be set");
    }

    #[test]
    fn capability_path_not_found_returns_error() {
        let cap = IndexerModule::build_cap(&IndexConfig::in_memory()).expect("build_cap");
        let result = cap.index(Path::new("/nonexistent/path/xyz"), "demo", false);
        assert!(result.is_err(), "path not found should error");
        let err = result.unwrap_err();
        assert!(
            matches!(err, IndexError::PathNotFound(_)),
            "expected PathNotFound, got {err:?}"
        );
        assert_eq!(err.exit_code(), 1, "PRD §4.1.6: path not found → exit 1");
    }

    /// Verify the full AsyncKit registration flow works end-to-end.
    #[tokio::test]
    async fn kit_registration_flow() {
        let mut kit = AsyncKit::new();
        kit.set_config(IndexConfig::in_memory());
        kit.register::<IndexerModule>()
            .expect("register::<IndexerModule>");
        let kit = kit.build().await.expect("build");

        assert!(kit.contains::<IndexerModule>());

        let _required = kit
            .require::<IndexerModule>()
            .expect("require::<IndexerModule>");
    }

    /// `index_incremental` on an empty directory returns zero files indexed.
    #[test]
    fn capability_index_incremental_empty_dir_returns_zero_files() {
        let cap = IndexerModule::build_cap(&IndexConfig::in_memory()).expect("build_cap");
        let tmp = TempDir::new().unwrap();
        let result = cap
            .index_incremental(tmp.path(), "empty", false)
            .expect("index_incremental on empty dir");
        assert_eq!(result.files_indexed, 0, "empty dir → 0 files indexed");
        assert_eq!(result.files_skipped, 0);
    }
}
