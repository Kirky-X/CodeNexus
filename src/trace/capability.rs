// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! TraceEngine capability trait (T6/unified-architecture Phase 2, Task 2.3).
//!
//! Defines [`TraceEngine`], the capability trait object stored in
//! [`Kit`](crate::kit::Kit) under [`TraceKey`](crate::kit::TraceKey). The
//! concrete impl (Task 2.10) wraps [`TraceFacade`] over an owned/shared
//! [`Graph`].
//!
//! [`TraceFacade`]: super::TraceFacade
//! [`Graph`]: crate::model::Graph

use super::error::TraceError;
use super::facade::TraceType;
use super::TraceResult;

/// Capability trait for the Trace subsystem (call-graph + data-flow BFS).
///
/// Stored in [`Kit`](crate::kit::Kit) as `Arc<dyn TraceEngine>` under
/// [`TraceKey`](crate::kit::TraceKey). Requires `StorageKey` + `ResolverKey`.
/// The concrete impl (Task 2.10) wraps [`TraceFacade`](super::TraceFacade)
/// over an owned/shared [`Graph`](crate::model::Graph) — the facade currently
/// borrows a `&Graph`, so the migration owns or shares one.
pub trait TraceEngine: Send + Sync {
    /// Resolves `symbol` to a node id and dispatches to the appropriate
    /// tracer(s) based on `trace_type`. `depth` must be at least 1.
    fn trace(
        &self,
        symbol: &str,
        trace_type: TraceType,
        depth: usize,
    ) -> std::result::Result<TraceResult, TraceError>;
}

/// Compile-time assertion that `TraceEngine` is object-safe and `Send + Sync`.
#[cfg(test)]
const _: () = {
    fn _assert_object_safe(_: &dyn TraceEngine) {}
    fn _assert_send_sync<T: Send + Sync + ?Sized>() {}
    fn _check() {
        _assert_send_sync::<dyn TraceEngine>();
        let _ = _assert_object_safe;
    }
};
