// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Resolver capability trait (T6/unified-architecture Phase 2, Task 2.3).
//!
//! Defines [`Resolver`], the capability trait object stored in
//! [`Kit`](crate::kit::Kit) under [`ResolverKey`](crate::kit::ResolverKey).
//! The concrete impl (Task 2.8) wraps the free functions
//! [`build_symbol_table`](super::build_symbol_table) and
//! [`resolve_all`](super::resolve_all).

use crate::ir::ExtractResult;
use crate::model::{Edge, Graph};

use super::symbol_table::ProjectSymbolTable;

/// Capability trait for the Resolver subsystem (calls + dataflow + FFI).
///
/// Stored in [`Kit`](crate::kit::Kit) as `Arc<dyn Resolver>` under
/// [`ResolverKey`](crate::kit::ResolverKey). Requires `StorageKey`. The
/// concrete impl (Task 2.8) delegates to
/// [`build_symbol_table`](super::build_symbol_table) and
/// [`resolve_all`](super::resolve_all).
pub trait Resolver: Send + Sync {
    /// Builds a project-level symbol table from extraction results.
    fn build_symbol_table(&self, results: &[ExtractResult], project: &str)
        -> ProjectSymbolTable;

    /// Resolves all symbols (calls + dataflows + FFI), adding edges to
    /// `graph`. Returns the resolved edges.
    fn resolve_all(
        &self,
        results: &[ExtractResult],
        symbol_table: &ProjectSymbolTable,
        project: &str,
        graph: &mut Graph,
    ) -> Vec<Edge>;
}

/// Compile-time assertion that `Resolver` is object-safe and `Send + Sync`.
#[cfg(test)]
const _: () = {
    fn _assert_object_safe(_: &dyn Resolver) {}
    fn _assert_send_sync<T: Send + Sync + ?Sized>() {}
    fn _check() {
        _assert_send_sync::<dyn Resolver>();
        let _ = _assert_object_safe;
    }
};
