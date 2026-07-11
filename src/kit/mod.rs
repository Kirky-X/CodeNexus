// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Kit — unified capability & configuration registry (T6/unified-architecture
//! Phase 2).
//!
//! This module re-exports the trait-kit 0.2.4 `AsyncKit` API (see design.md D5)
//! and the 9 subsystem module types used for capability lookup.
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
//! Capability and config lookup uses the module type directly (e.g.,
//! `kit.require::<StorageModule>()` and `kit.set_config::<StorageConfig>()`).

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
#[cfg(feature = "cache")]
pub use crate::cache::CacheModule;

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
