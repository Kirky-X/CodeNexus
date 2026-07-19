// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! CacheStore capability trait — abstract cache interface for storage,
//! query, AST, and embed caching (T017, v0.3.3).
//!
//! Stored in AsyncKit as `Arc<dyn CacheStore>` via [`CacheModule`]. The
//! trait is intentionally synchronous (no `async` methods) because the
//! downstream call sites that consume it (`hash_file`, `parse_file`,
//! `execute_cypher`) are themselves synchronous. The underlying oxcache
//! `MokaMemoryBackend` resolves sync access via `sync_block_on`, which
//! auto-detects the tokio runtime (multi-thread `block_in_place` or
//! lazily-created current-thread runtime).

/// Cache key type — all cache operations use string keys.
///
/// Callers are responsible for constructing meaningful keys (e.g., file
/// paths, BLAKE3 hashes of query strings, content hashes). This is a
/// type alias rather than a newtype to keep the ergonomics of `&str` and
/// `String` at call sites.
pub type CacheKey = String;

/// Capability trait for the Cache subsystem.
///
/// Provides `get` / `set` / `invalidate_all` operations for
/// content-addressed caching. Values are raw bytes (`Vec<u8>`) — callers
/// are responsible for serialization / deserialization.
///
/// # Errors
///
/// All methods are infallible (do not return `Result`):
/// - [`get`](CacheStore::get) returns `None` on cache miss *or* internal
///   error (errors are logged via `tracing::warn!` and surfaced as misses).
/// - [`set`](CacheStore::set) and [`invalidate_all`](CacheStore::invalidate_all)
///   log errors via `tracing::warn!` but do not propagate them. This
///   matches cache semantics — a failed write should not crash the
///   caller; the worst case is a cache miss on the next read.
pub trait CacheStore: Send + Sync {
    /// Retrieve a cached value by key.
    ///
    /// Returns `None` on cache miss or internal error. Callers should
    /// treat `None` as "compute and [`set`](Self::set)".
    fn get(&self, key: &str) -> Option<Vec<u8>>;

    /// Store a value in the cache. Overwrites any prior value for the
    /// same key.
    fn set(&self, key: &str, val: Vec<u8>);

    /// Invalidate all cached entries.
    ///
    /// Called after graph mutations (index / clean) to ensure query
    /// consistency. The cache is also invalidated implicitly when entries
    /// expire via TTL, but explicit invalidation is required for
    /// correctness after writes.
    fn invalidate_all(&self);
}
