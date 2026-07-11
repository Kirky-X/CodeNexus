// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Kit — unified capability & configuration registry (T6/unified-architecture
//! Phase 2).
//!
//! This module re-exports the trait-kit 0.2.4 `AsyncKit` API (see design.md D5)
//! and declares the `*Key` / `*ConfigKey` marker types that historically
//! identified each subsystem's capability and config in the Kit registry.
//!
//! ## AsyncKit vs. Kit
//!
//! trait-kit 0.2.4's synchronous `Kit` uses `RefCell` internally and is
//! therefore `!Send + !Sync`. CodeNexus stores the kit in a
//! `static Mutex<Option<Arc<AsyncKit<Ready>>>>` (see `service::runtime`),
//! which requires `Send + Sync`. We therefore use `AsyncKit`, which is backed
//! by `Arc<RwLock<...>>` and implements `Send + Sync`. Only `build()` is
//! async; `require::<M>()` and `config::<C>()` are synchronous methods.
//!
//! ## Capability vs. Config keys (legacy markers)
//!
//! The `*Key` and `*ConfigKey` structs are retained as pure type identifiers
//! for backwards-compatibility documentation. Capability lookup is now via
//! the module type (e.g., `kit.require::<StorageModule>()`). These marker
//! structs implement no trait and are not used at runtime.

// `trait-kit` is a hard dependency (Task 2.16 removed the in-tree shim once
// all modules migrated to `build_kit`). No feature gating needed.
extern crate trait_kit;

// Bootstrap (Task 2.13) — wires all 9 modules into a fresh AsyncKit in
// dependency order. Re-exported at the kit module root so callers can
// write `codenexus::kit::build_kit` and `codenexus::kit::KitBootstrapConfig`.
pub mod bootstrap;
pub use bootstrap::{build_kit, KitBootstrapConfig};

// Re-export the trait-kit 0.2.4 AsyncKit API. These are the canonical
// imports for call sites that need to interact with the Kit registry.
// Note: we import `AsyncReady`/`AsyncUnbuilt` (the async-feature re-exports),
// NOT `Ready`/`Unbuilt` (the synchronous Kit markers). trait-kit exports the
// async markers via `pub use async_kit::{Ready as AsyncReady, Unbuilt as
// AsyncUnbuilt}` — the two `Ready` types are distinct structs (sync `Ready`
// in `kit.rs` vs async `Ready` in `async_kit.rs`). Using the wrong one causes
// a type mismatch: `AsyncKit::build()` returns `AsyncKit<async_kit::Ready>`.
pub use trait_kit::core::error::KitError;
pub use trait_kit::core::meta::{AsyncAutoBuilder, ModuleMeta};
pub use trait_kit::kit::{AsyncKit, AsyncReady, AsyncUnbuilt};

// Re-export the 9 module types so call sites can write
// `use crate::kit::{AsyncKit, StorageModule, TraceModule}` instead of
// importing each module from its own crate path. This mirrors the
// historical convenience where `*Key` types lived in `crate::kit`.
pub use crate::index::IndexerModule;
pub use crate::parse::{ExtractorRegistryModule, ParserFactoryModule};
pub use crate::query::QueryModule;
pub use crate::resolve::ResolverModule;
pub use crate::storage::StorageModule;
pub use crate::trace::TraceModule;
#[cfg(feature = "daemon")]
pub use crate::daemon::DaemonModule;
#[cfg(feature = "embed")]
pub use crate::embed::EmbedModule;

// ---------------------------------------------------------------------------
// Capability key markers (pure type identifiers — no trait impl)
// ---------------------------------------------------------------------------
//
// Retained as documentation anchors. Capability lookup now uses the module
// type directly (e.g., `kit.require::<StorageModule>()`). These structs
// implement no trait and are not used at runtime.

/// Capability key marker for the Storage subsystem.
///
/// Use [`StorageModule`](crate::storage::StorageModule) for capability lookup.
#[allow(dead_code)]
pub struct StorageKey;

/// Capability key marker for the Parser subsystem.
///
/// Use [`ParserFactoryModule`](crate::parse::ParserFactoryModule) for
/// capability lookup.
#[allow(dead_code)]
pub struct ParserKey;

/// Capability key marker for the Extractor registry.
///
/// Use [`ExtractorRegistryModule`](crate::parse::ExtractorRegistryModule) for
/// capability lookup.
#[allow(dead_code)]
pub struct ExtractorKey;

/// Capability key marker for the Indexer subsystem.
///
/// Use [`IndexerModule`](crate::index::IndexerModule) for capability lookup.
#[allow(dead_code)]
pub struct IndexerKey;

/// Capability key marker for the Resolver subsystem.
///
/// Use [`ResolverModule`](crate::resolve::ResolverModule) for capability
/// lookup.
#[allow(dead_code)]
pub struct ResolverKey;

/// Capability key marker for the Query subsystem.
///
/// Use [`QueryModule`](crate::query::QueryModule) for capability lookup.
#[allow(dead_code)]
pub struct QueryKey;

/// Capability key marker for the Trace subsystem.
///
/// Use [`TraceModule`](crate::trace::TraceModule) for capability lookup.
#[allow(dead_code)]
pub struct TraceKey;

/// Capability key marker for the Daemon subsystem (feature-gated).
///
/// Use [`DaemonModule`](crate::daemon::DaemonModule) for capability lookup.
#[cfg(feature = "daemon")]
#[allow(dead_code)]
pub struct DaemonKey;

/// Capability key marker for the Embed subsystem (feature-gated).
///
/// Use [`EmbedModule`](crate::embed::EmbedModule) for capability lookup.
#[cfg(feature = "embed")]
#[allow(dead_code)]
pub struct EmbedKey;

// ---------------------------------------------------------------------------
// Config key markers (pure type identifiers — no trait impl)
// ---------------------------------------------------------------------------
//
// Config values are now stored/retrieved via `AsyncKit::set_config::<C>()` /
// `AsyncKit::config::<C>()`, keyed by `TypeId`. These marker structs are
// retained for documentation and are not used at runtime.

/// Config key marker for the Storage subsystem (`StorageConfig`).
#[allow(dead_code)]
pub struct StorageConfigKey;

/// Config key marker for the Indexer subsystem (`IndexConfig`).
#[allow(dead_code)]
pub struct IndexConfigKey;

/// Config key marker for the Query subsystem (`QueryConfig`).
#[allow(dead_code)]
pub struct QueryConfigKey;

/// Config key marker for the Trace subsystem (`TraceConfig`).
#[allow(dead_code)]
pub struct TraceConfigKey;

/// Config key marker for the Daemon subsystem (`DaemonConfig`).
#[cfg(feature = "daemon")]
#[allow(dead_code)]
pub struct DaemonConfigKey;

/// Config key marker for the Embed subsystem (`EmbeddingConfig`).
#[cfg(feature = "embed")]
#[allow(dead_code)]
pub struct EmbedConfigKey;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// `AsyncKit::new()` creates an empty `AsyncKit<Unbuilt>` without
    /// panicking. This is a smoke test that the trait-kit 0.2.4 wiring loads
    /// correctly after the migration from 0.1.0.
    #[test]
    fn async_kit_new_creates_empty_unbuilt_kit() {
        let kit = AsyncKit::new();
        // A fresh AsyncKit<Unbuilt> should be creatable. We don't call
        // build() here because that requires an async runtime — the
        // bootstrap tests in bootstrap.rs exercise the full build path.
        let _ = kit;
    }
}
