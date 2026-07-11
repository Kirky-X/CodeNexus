// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Cache module — multi-level cache integration via oxcache (T017,
//! v0.3.3-sibling-crate-optimization Phase 3).
//!
//! Feature-gated behind the `cache` cargo feature. When enabled, a
//! [`CacheModule`] is registered in the AsyncKit during `build_kit`,
//! exposing an `Arc<dyn CacheStore>` capability for content-addressed
//! caching across subsystems (file hashes, Cypher results, AST, embeddings).
//!
//! # Architecture
//!
//! ```text
//! AsyncKit::build_kit
//!   └── CacheModule (AsyncAutoBuilder)
//!        └── OxcacheStore (dyn CacheStore)
//!             └── oxcache::Cache<String, Vec<u8>> (sync_mode = true)
//!                  └── MokaMemoryBackend (L1 moka in-memory cache)
//! ```
//!
//! The sync [`CacheStore`] trait is a thin wrapper over oxcache's
//! `get_bytes_sync` / `set_bytes_sync` / `clear_sync` API. Callers are
//! responsible for serializing cached values to `Vec<u8>` — the cache is
//! agnostic to the value format.

pub mod capability;
pub mod module;

pub use capability::{CacheKey, CacheStore};
pub use module::{CacheConfig, CacheError, CacheModule};
