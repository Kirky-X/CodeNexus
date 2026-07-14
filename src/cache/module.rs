// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! trait-kit module for the Cache subsystem (T017,
//! v0.3.3-sibling-crate-optimization Phase 3).
//!
//! Implements [`ModuleMeta`] + [`AsyncAutoBuilder`] for [`CacheModule`],
//! wiring an oxcache moka memory cache into the unified Kit registry as
//! `Arc<dyn CacheStore>` under [`CacheModule`](crate::cache::CacheModule).
//!
//! # Design
//!
//! `CacheModule` wraps `oxcache::cache::Cache<String, Vec<u8>>` built with
//! `sync_mode(true)`. The sync byte-level API (`get_bytes_sync` /
//! `set_bytes_sync` / `clear_sync`) maps directly to the [`CacheStore`]
//! trait methods.
//!
//! # Sync access
//!
//! `oxcache::cache::Cache` is async-first; sync access requires
//! `sync_mode(true)` on the builder. The underlying `MokaMemoryBackend`
//! uses `sync_block_on` which auto-detects the tokio runtime:
//! - Multi-thread runtime: uses `block_in_place`
//! - No runtime / current-thread: lazily creates a current-thread runtime
//!
//! # Dependency note
//!
//! The cache is a leaf module (no upstream dependencies). It does not
//! depend on `StorageModule` or any other subsystem — the moka memory
//! cache is self-contained. Registered last in `build_kit` (after Embed).

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::kit::{AsyncAutoBuilder, AsyncKit, ModuleMeta};

use super::capability::CacheStore;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Error type for [`CacheModule`] operations.
///
/// Used as `AsyncAutoBuilder::Error` for `CacheModule`. Cache runtime
/// errors (get/set failures) are NOT propagated via this type — they are
/// logged and surfaced as cache misses (see [`CacheStore`] trait docs).
/// This type covers only build/initialization failures.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// `AsyncKit::config::<CacheConfig>()` failed (config not set or
    /// type-mismatched). The cache module requires `kit.set_config(...)`
    /// before `kit.register::<CacheModule>()`.
    #[error("cache config error: {0}")]
    Config(String),

    /// `oxcache::Cache::builder().build()` failed. This typically
    /// indicates a backend construction error (e.g., moka capacity = 0).
    #[error("cache build failed: {0}")]
    BuildFailed(String),
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for [`CacheModule`] (T017).
///
/// Stored in Kit via `AsyncKit::set_config` and read in
/// [`AsyncAutoBuilder::build`]. Defaults to 10,000 entries (matching
/// oxcache's `MokaMemoryBackend` default).
///
/// # Example
///
/// ```ignore
/// use codenexus::cache::CacheConfig;
///
/// let config = CacheConfig::default().with_capacity(5_000);
/// ```
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Max entries held by the L1 moka memory backend.
    /// Default: 10,000 (matches oxcache's `MokaMemoryBackend` default).
    pub capacity: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self { capacity: 10_000 }
    }
}

impl CacheConfig {
    /// Creates a config with the default capacity (10,000 entries).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the max capacity.
    #[must_use]
    pub fn with_capacity(mut self, capacity: u64) -> Self {
        self.capacity = capacity;
        self
    }
}

// ---------------------------------------------------------------------------
// Module (ModuleMeta + AsyncAutoBuilder)
// ---------------------------------------------------------------------------

/// trait-kit module tag for the Cache subsystem (T017).
///
/// Zero-sized marker — construction logic lives in
/// [`CacheModule::build_cap`] (called from the [`AsyncAutoBuilder`] impl).
/// Register the capability in Kit via:
///
/// ```ignore
/// use codenexus::kit::{AsyncKit, CacheModule};
/// use codenexus::cache::CacheConfig;
///
/// let mut kit = AsyncKit::new();
/// kit.set_config(CacheConfig::default());
/// kit.register::<CacheModule>()?;
/// let kit = kit.build().await?;
/// let cache = kit.require::<CacheModule>()?;
/// ```
pub struct CacheModule;

impl ModuleMeta for CacheModule {
    const NAME: &'static str = "cache";
    fn dependencies() -> &'static [(&'static str, TypeId)] {
        &[]
    }
}

impl AsyncAutoBuilder for CacheModule {
    type Capability = Arc<dyn CacheStore>;
    type Error = CacheError;

    fn build<'a>(
        kit: &'a AsyncKit,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Capability, Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            let config = kit
                .config::<CacheConfig>()
                .map_err(|e| CacheError::Config(e.to_string()))?;
            Self::build_cap(&config).await
        })
    }
}

impl CacheModule {
    /// Constructs an [`OxcacheStore`] from the given config.
    ///
    /// Shared between [`AsyncAutoBuilder::build`] and tests so that
    /// capability-level tests can run with a single async call.
    ///
    /// # Errors
    ///
    /// Returns [`CacheError::BuildFailed`] if oxcache's `Cache::build()`
    /// fails (e.g., invalid capacity).
    pub(crate) async fn build_cap(config: &CacheConfig) -> Result<Arc<dyn CacheStore>, CacheError> {
        let cache = oxcache::Cache::builder()
            .capacity(config.capacity)
            .sync_mode(true)
            .build()
            .await
            .map_err(|e| CacheError::BuildFailed(e.to_string()))?;
        Ok(Arc::new(OxcacheStore::new(cache)))
    }
}

// ---------------------------------------------------------------------------
// Concrete dyn CacheStore implementation
// ---------------------------------------------------------------------------

/// Concrete implementation of [`dyn CacheStore`] wrapping an
/// `oxcache::cache::Cache<String, Vec<u8>>`.
///
/// The inner `Cache` is built with `sync_mode(true)`, enabling the sync
/// byte-level API (`get_bytes_sync` / `set_bytes_sync` / `clear_sync`).
/// The `Cache<K,V>` type is `Send + Sync` because all of its fields
/// (`Arc<dyn CacheBackend>`, `Option<Arc<dyn SyncCacheBackend>>`, etc.)
/// are `Send + Sync` under the `minimal` feature set.
struct OxcacheStore {
    /// The oxcache Cache instance. Built with `sync_mode(true)` so
    /// `backend_sync` is `Some`, enabling the `_sync` methods.
    inner: oxcache::Cache<String, Vec<u8>>,
}

impl OxcacheStore {
    /// Creates a new `OxcacheStore` wrapping the given oxcache `Cache`.
    fn new(cache: oxcache::Cache<String, Vec<u8>>) -> Self {
        Self { inner: cache }
    }
}

impl CacheStore for OxcacheStore {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        match self.inner.get_bytes_sync(key) {
            Ok(val) => val,
            Err(e) => {
                tracing::warn!(key = %key, error = %e, "cache get failed");
                None
            }
        }
    }

    fn set(&self, key: &str, val: Vec<u8>) {
        if let Err(e) = self.inner.set_bytes_sync(key, val, None) {
            tracing::warn!(key = %key, error = %e, "cache set failed");
        }
    }

    fn invalidate_all(&self) {
        if let Err(e) = self.inner.clear_sync() {
            tracing::warn!(error = %e, "cache invalidate_all failed");
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{AsyncKit, CacheModule};

    /// Builds a CacheModule capability with default config and returns it.
    async fn build_cache() -> Arc<dyn CacheStore> {
        CacheModule::build_cap(&CacheConfig::default())
            .await
            .expect("CacheModule::build_cap")
    }

    #[test]
    fn cache_config_default_capacity() {
        let config = CacheConfig::default();
        assert_eq!(config.capacity, 10_000);
    }

    #[test]
    fn cache_config_with_capacity_overrides_default() {
        let config = CacheConfig::new().with_capacity(5_000);
        assert_eq!(config.capacity, 5_000);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn build_returns_send_sync_capability() {
        let cap = build_cache().await;
        fn _assert_send_sync<T: Send + Sync>(_: &T) {}
        _assert_send_sync(&cap);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn capability_get_miss_returns_none() {
        let cap = build_cache().await;
        assert!(cap.get("nonexistent").is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn capability_set_then_get_returns_value() {
        let cap = build_cache().await;
        cap.set("key1", b"value1".to_vec());
        assert_eq!(cap.get("key1"), Some(b"value1".to_vec()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn capability_set_overwrites_existing() {
        let cap = build_cache().await;
        cap.set("k", b"v1".to_vec());
        cap.set("k", b"v2".to_vec());
        assert_eq!(cap.get("k"), Some(b"v2".to_vec()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn capability_invalidate_all_clears_entries() {
        let cap = build_cache().await;
        cap.set("k1", b"v1".to_vec());
        cap.set("k2", b"v2".to_vec());
        cap.invalidate_all();
        assert!(cap.get("k1").is_none());
        assert!(cap.get("k2").is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn capability_set_empty_value() {
        let cap = build_cache().await;
        let empty: Vec<u8> = vec![];
        cap.set("empty", empty.clone());
        assert_eq!(cap.get("empty"), Some(empty));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn capability_set_large_value() {
        let cap = build_cache().await;
        let large = vec![0xAB; 1024 * 100]; // 100KB
        cap.set("large", large.clone());
        assert_eq!(cap.get("large"), Some(large));
    }

    /// Verify the full AsyncKit registration flow works end-to-end.
    #[tokio::test(flavor = "multi_thread")]
    async fn kit_registration_flow() {
        let mut kit = AsyncKit::new();
        kit.set_config(CacheConfig::default());
        kit.register::<CacheModule>()
            .expect("register::<CacheModule>");
        let kit = kit.build().await.expect("build");

        assert!(kit.contains::<CacheModule>(), "CacheModule missing");

        let cache = kit
            .require::<CacheModule>()
            .expect("require::<CacheModule>");
        cache.set("k", b"v".to_vec());
        assert_eq!(cache.get("k"), Some(b"v".to_vec()));
    }
}
