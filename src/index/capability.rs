// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Indexer capability trait (T6/unified-architecture Phase 2, Task 2.3).
//!
//! Defines [`Indexer`], the capability trait object stored in
//! [`Kit`](crate::kit::Kit) under [`IndexerKey`](crate::kit::IndexerKey). The
//! concrete impl (Task 2.7) wraps [`IndexFacade`].
//!
//! [`IndexFacade`]: super::IndexFacade

use std::path::Path;

use super::error::IndexError;
use super::pipeline::IndexResult;

/// Capability trait for the Indexer subsystem (index pipeline facade).
///
/// Stored in [`Kit`](crate::kit::Kit) as `Arc<dyn Indexer>` under
/// [`IndexerKey`](crate::kit::IndexerKey). Requires `StorageKey` +
/// `ExtractorKey`. The concrete impl (Task 2.7) wraps
/// [`IndexFacade`](super::IndexFacade).
pub trait Indexer: Send + Sync {
    /// Runs the full index pipeline (no incremental diffing).
    fn index(
        &self,
        path: &Path,
        project_name: &str,
        force: bool,
    ) -> std::result::Result<IndexResult, IndexError>;

    /// Runs the incremental index pipeline (only changed files are parsed).
    fn index_incremental(
        &self,
        path: &Path,
        project_name: &str,
        force: bool,
    ) -> std::result::Result<IndexResult, IndexError>;

    /// Runs the RAM-first index pipeline (H15/D9).
    ///
    /// LZ4-compresses source files into memory, parses from memory, then
    /// performs a single `COPY FROM` dump. Use for small-to-medium
    /// repositories (< 1 GB source). The default [`index`](Self::index)
    /// streaming path is retained for large repositories.
    fn index_ram_first(
        &self,
        path: &Path,
        project_name: &str,
        force: bool,
    ) -> std::result::Result<IndexResult, IndexError>;
}

/// Compile-time assertion that `Indexer` is object-safe and `Send + Sync`.
#[cfg(test)]
const _: () = {
    fn _assert_object_safe(_: &dyn Indexer) {}
    fn _assert_send_sync<T: Send + Sync + ?Sized>() {}
    fn _check() {
        _assert_send_sync::<dyn Indexer>();
        let _ = _assert_object_safe;
    }
};
