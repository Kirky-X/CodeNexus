// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Kit — unified capability & configuration registry (T6/unified-architecture
//! Phase 2).
//!
//! This module declares the [`CapabilityKey`] and [`ConfigKey`] types that
//! identify each subsystem's capability in the [`Kit`] registry. Subsystems
//! (Storage, Parser, Extractor, Indexer, Resolver, Query, Trace, Daemon,
//! Embed) are migrated one-by-one in Tasks 2.4–2.12 to expose their facades
//! as `Module`s registered under these keys.
//!
//! ## Feature gating
//!
//! When the `trait-kit` feature is enabled (default for all presets), the real
//! [`trait_kit`] crate provides the `Module`/`ModuleBuilder`/`CapabilityKey`/
//! `ConfigKey`/`Kit`/`ConfigHandle` API. When disabled, the in-tree
//! [`shim`] module mirrors the same API so the codebase keeps compiling —
//! this is the escape hatch if `trait-kit` ever stops publishing
//! (design.md §2.4).
//!
//! ## Capability vs. Config keys
//!
//! - **Capability keys** identify a `dyn Trait` object stored as
//!   `Arc<dyn Trait>` in Kit (e.g. `StorageKey` → `dyn Storage`).
//! - **Config keys** identify a `Sized + Send + Sync` config value stored
//!   behind a lock-free `ConfigHandle<T>` (e.g. `DaemonConfigKey` →
//!   `DaemonConfig`), enabling hot reconfiguration without rebuilding the
//!   module.
//!
//! Task 2.1 only declares the key types; the real capability traits are
//! defined in Task 2.3 (`src/storage/capability.rs`, etc.) and the keys'
//! `Capability` associated types are tightened to reference them at that
//! point. Until then, keys use `dyn Send + Sync` as a placeholder so the
//! module compiles standalone.

// Pull in the real `trait_kit` crate when the feature is on; otherwise defer
// to the in-tree shim (Task 2.2 populates `shim.rs`). The shim re-exports the
// same path `trait_kit::...` so downstream code is feature-agnostic.
#[cfg(feature = "trait-kit")]
extern crate trait_kit;

#[cfg(not(feature = "trait-kit"))]
#[doc(hidden)]
pub mod shim;
#[cfg(not(feature = "trait-kit"))]
pub use shim as trait_kit;

// Re-export the most commonly used trait-kit items so call sites can write
// `use crate::kit::{CapabilityKey, ConfigKey, Kit}` instead of the longer
// `trait_kit::core::...` path. Both the real crate and the shim expose the
// same names.
pub use trait_kit::core::builder::{ModuleBuilder, WithConfig, WithRequirements};
pub use trait_kit::core::capability::CapabilityKey;
pub use trait_kit::core::config::{ConfigHandle, ConfigKey};
pub use trait_kit::core::marker::{NoConfig, NoRequirements};
pub use trait_kit::core::module::Module;
pub use trait_kit::kit::builder::{IntoKitModuleBuilder, KitModuleBuilder};
pub use trait_kit::kit::{Kit, KitError};

// ---------------------------------------------------------------------------
// Capability keys
// ---------------------------------------------------------------------------
//
// Each subsystem gets one zero-sized marker type implementing `CapabilityKey`.
// The `Capability` associated type is the trait object stored in Kit as
// `Arc<dyn ...>`. The 7 core keys (Storage, Parser, Extractor, Indexer,
// Resolver, Query, Trace) reference the real capability traits defined in
// Task 2.3; `DaemonKey` and `EmbedKey` retain a `dyn Send + Sync` placeholder
// until their capability traits land in Tasks 2.11/2.12.

/// Capability key for the Storage subsystem (LadybugDB connection pool).
///
/// Registered by `StorageModule` (Task 2.4). Capability will be
/// `dyn storage::capability::Storage` once Task 2.3 lands.
pub struct StorageKey;

/// Capability key for the Parser subsystem (`ParserPool`).
///
/// Registered by `ParserFactoryModule` (Task 2.5). Capability will be
/// `dyn parse::capability::ParserRegistry`.
pub struct ParserKey;

/// Capability key for the Extractor registry (per-language dispatch).
///
/// Registered by `ExtractorRegistryModule` (Task 2.6). Capability will be
/// `dyn parse::capability::ExtractorRegistry`. Requires `ParserKey`.
pub struct ExtractorKey;

/// Capability key for the Indexer subsystem (pipeline facade).
///
/// Registered by `IndexerModule` (Task 2.7). Capability will be
/// `dyn index::capability::Indexer`. Requires `StorageKey` + `ExtractorKey`.
pub struct IndexerKey;

/// Capability key for the Resolver subsystem (calls + dataflow + ffi).
///
/// Registered by `ResolverModule` (Task 2.8). Capability will be
/// `dyn resolve::capability::Resolver`. Requires `StorageKey`.
pub struct ResolverKey;

/// Capability key for the Query subsystem (cypher + structured + fulltext).
///
/// Registered by `QueryModule` (Task 2.9). Capability will be
/// `dyn query::capability::QueryEngine`. Requires `StorageKey`.
pub struct QueryKey;

/// Capability key for the Trace subsystem.
///
/// Registered by `TraceModule` (Task 2.10). Capability will be
/// `dyn trace::capability::TraceEngine`. Requires `StorageKey` + `ResolverKey`.
pub struct TraceKey;

/// Capability key for the Daemon subsystem (file watcher).
///
/// Only available when the `daemon` feature is enabled. Registered by
/// `DaemonModule` (Task 2.11). Capability will be
/// `dyn daemon::capability::Daemon`. Requires `StorageKey` + `IndexerKey`.
#[cfg(feature = "daemon")]
pub struct DaemonKey;

/// Capability key for the Embed subsystem (vector embeddings).
///
/// Only available when the `embed` feature is enabled. Registered by
/// `EmbedModule` (Task 2.12). Capability will be
/// `dyn embed::capability::Embedder`. Requires `StorageKey`.
#[cfg(feature = "embed")]
pub struct EmbedKey;

// --- CapabilityKey impls --------------------------------------------------
//
// NOTE: The 7 core keys now reference real capability traits (Task 2.3).
// `DaemonKey` and `EmbedKey` retain `dyn Send + Sync` as a placeholder until
// their capability traits are defined in Tasks 2.11/2.12.

impl CapabilityKey for StorageKey {
    type Capability = dyn crate::storage::capability::Storage;
    const NAME: &'static str = "storage";
}

impl CapabilityKey for ParserKey {
    type Capability = dyn crate::parse::capability::ParserRegistry;
    const NAME: &'static str = "parser";
}

impl CapabilityKey for ExtractorKey {
    type Capability = dyn crate::parse::capability::ExtractorRegistry;
    const NAME: &'static str = "extractor";
}

impl CapabilityKey for IndexerKey {
    type Capability = dyn crate::index::capability::Indexer;
    const NAME: &'static str = "indexer";
}

impl CapabilityKey for ResolverKey {
    type Capability = dyn crate::resolve::capability::Resolver;
    const NAME: &'static str = "resolver";
}

impl CapabilityKey for QueryKey {
    type Capability = dyn crate::query::capability::QueryEngine;
    const NAME: &'static str = "query";
}

impl CapabilityKey for TraceKey {
    type Capability = dyn crate::trace::capability::TraceEngine;
    const NAME: &'static str = "trace";
}

#[cfg(feature = "daemon")]
impl CapabilityKey for DaemonKey {
    type Capability = dyn Send + Sync;
    const NAME: &'static str = "daemon";
}

#[cfg(feature = "embed")]
impl CapabilityKey for EmbedKey {
    type Capability = dyn Send + Sync;
    const NAME: &'static str = "embed";
}

// ---------------------------------------------------------------------------
// Config keys
// ---------------------------------------------------------------------------
//
// Config keys identify a `Sized + Send + Sync` config value stored behind a
// lock-free `ConfigHandle<T>`. This enables hot reconfiguration (e.g. the
// daemon's `--debounce-ms` updates `DaemonConfig` without rebuilding the
// watcher — design.md §2.3). The actual config structs (`StorageConfig`,
// `IndexConfig`, `DaemonConfig`, `EmbedConfig`) are defined alongside their
// modules in Tasks 2.4–2.12; until then, a placeholder `()` is used.

/// Config key for the Storage subsystem (`StorageConfig { db_path, pool_size }`).
///
/// Registered by `StorageModule` (Task 2.4).
pub struct StorageConfigKey;

/// Config key for the Indexer subsystem (`IndexConfig { db_path }`).
///
/// Registered by `IndexerModule` (Task 2.7). Tightened to `IndexConfig` in
/// Task 2.7 (previously a `()` placeholder).
pub struct IndexConfigKey;

/// Config key for the Query subsystem (`QueryConfig { db_path }`).
///
/// Registered by `QueryModule` (Task 2.9). Tightened to `QueryConfig` in
/// Task 2.9 (previously a `()` placeholder).
pub struct QueryConfigKey;

/// Config key for the Daemon subsystem (`DaemonConfig { debounce_ms }`).
///
/// Only available when the `daemon` feature is enabled. Registered by
/// `DaemonModule` (Task 2.11). Updated hot via `ConfigHandle::set` when the
/// user changes `--debounce-ms`.
#[cfg(feature = "daemon")]
pub struct DaemonConfigKey;

/// Config key for the Embed subsystem (`EmbedConfig { endpoint }`).
///
/// Only available when the `embed` feature is enabled. Registered by
/// `EmbedModule` (Task 2.12).
#[cfg(feature = "embed")]
pub struct EmbedConfigKey;

// --- ConfigKey impls ------------------------------------------------------
//
// NOTE: `StorageConfigKey` is tightened to `StorageConfig` (Task 2.4),
// `IndexConfigKey` to `IndexConfig` (Task 2.7), and `QueryConfigKey` to
// `QueryConfig` (Task 2.9). `DaemonConfigKey`/`EmbedConfigKey` retain `()` as
// a placeholder until Tasks 2.11/2.12 land.

impl ConfigKey for StorageConfigKey {
    type Config = crate::storage::module::StorageConfig;
    const NAME: &'static str = "storage_config";
}

impl ConfigKey for IndexConfigKey {
    type Config = crate::index::module::IndexConfig;
    const NAME: &'static str = "index_config";
}

impl ConfigKey for QueryConfigKey {
    type Config = crate::query::module::QueryConfig;
    const NAME: &'static str = "query_config";
}

#[cfg(feature = "daemon")]
impl ConfigKey for DaemonConfigKey {
    type Config = ();
    const NAME: &'static str = "daemon_config";
}

#[cfg(feature = "embed")]
impl ConfigKey for EmbedConfigKey {
    type Config = ();
    const NAME: &'static str = "embed_config";
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Every non-feature-gated capability key implements `CapabilityKey` and
    /// exposes a non-empty `NAME`. This guards against accidental rename or
    /// removal during the Phase 2 migration.
    #[test]
    fn capability_keys_have_names() {
        assert_eq!(StorageKey::NAME, "storage");
        assert_eq!(ParserKey::NAME, "parser");
        assert_eq!(ExtractorKey::NAME, "extractor");
        assert_eq!(IndexerKey::NAME, "indexer");
        assert_eq!(ResolverKey::NAME, "resolver");
        assert_eq!(QueryKey::NAME, "query");
        assert_eq!(TraceKey::NAME, "trace");
    }

    /// Feature-gated capability keys exist only when their feature is on.
    #[cfg(feature = "daemon")]
    #[test]
    fn daemon_capability_key_has_name() {
        assert_eq!(DaemonKey::NAME, "daemon");
    }

    #[cfg(feature = "embed")]
    #[test]
    fn embed_capability_key_has_name() {
        assert_eq!(EmbedKey::NAME, "embed");
    }

    /// Config keys implement `ConfigKey` with non-empty `NAME`.
    #[test]
    fn config_keys_have_names() {
        assert_eq!(StorageConfigKey::NAME, "storage_config");
        assert_eq!(IndexConfigKey::NAME, "index_config");
        assert_eq!(QueryConfigKey::NAME, "query_config");
    }

    #[cfg(feature = "daemon")]
    #[test]
    fn daemon_config_key_has_name() {
        assert_eq!(DaemonConfigKey::NAME, "daemon_config");
    }

    #[cfg(feature = "embed")]
    #[test]
    fn embed_config_key_has_name() {
        assert_eq!(EmbedConfigKey::NAME, "embed_config");
    }

    /// Kit can be instantiated and is empty (no capabilities registered yet).
    /// This is a smoke test that the trait-kit wiring (real crate or shim)
    /// loads correctly.
    #[test]
    fn kit_can_be_created() {
        let kit = Kit::new();
        // No capability should be registered in a fresh Kit.
        assert!(!kit.contains::<StorageKey>());
        assert!(!kit.contains::<ParserKey>());
    }
}
