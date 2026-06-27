// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! QueryEngine capability trait (T6/unified-architecture Phase 2, Task 2.3).
//!
//! Defines [`QueryEngine`], the capability trait object stored in
//! [`Kit`](crate::kit::Kit) under [`QueryKey`](crate::kit::QueryKey). The
//! concrete impl (Task 2.9) wraps [`QueryFacade`].
//!
//! [`QueryFacade`]: super::QueryFacade

use crate::model::NodeLabel;

use super::error::QueryError;
use super::{QueryResult, SearchResult};

/// Capability trait for the Query subsystem (cypher + structured + fulltext).
///
/// Stored in [`Kit`](crate::kit::Kit) as `Arc<dyn QueryEngine>` under
/// [`QueryKey`](crate::kit::QueryKey). Requires `StorageKey`. The concrete
/// impl (Task 2.9) wraps [`QueryFacade`](super::QueryFacade).
pub trait QueryEngine: Send + Sync {
    /// Executes a raw Cypher query.
    fn cypher(&self, query: &str) -> std::result::Result<QueryResult, QueryError>;

    /// General structured search by name (CONTAINS), sorted by relevance.
    fn search(
        &self,
        text: &str,
        project: Option<&str>,
        limit: usize,
    ) -> std::result::Result<Vec<SearchResult>, QueryError>;

    /// Returns all nodes of the given `label`, optionally filtered by project.
    fn search_by_type(
        &self,
        label: NodeLabel,
        project: Option<&str>,
        limit: usize,
    ) -> std::result::Result<Vec<SearchResult>, QueryError>;

    /// Returns all symbols located in `file_path`, optionally filtered.
    fn search_by_file(
        &self,
        file_path: &str,
        project: Option<&str>,
    ) -> std::result::Result<Vec<SearchResult>, QueryError>;

    /// BM25 full-text search (FTS extension when available, CONTAINS fallback).
    fn fulltext_search(
        &self,
        text: &str,
        project: Option<&str>,
        limit: usize,
    ) -> std::result::Result<Vec<SearchResult>, QueryError>;
}

/// Compile-time assertion that `QueryEngine` is object-safe and `Send + Sync`.
#[cfg(test)]
const _: () = {
    fn _assert_object_safe(_: &dyn QueryEngine) {}
    fn _assert_send_sync<T: Send + Sync + ?Sized>() {}
    fn _check() {
        _assert_send_sync::<dyn QueryEngine>();
        let _ = _assert_object_safe;
    }
};
