// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Integration test for `build_kit` (Task 2.15 / unified-architecture
//! Phase 2).
//!
//! Verifies spec `specs/trait-kit-unified-registry/spec.md` scenario
//! "All nine capability keys resolvable through Kit":
//!
//! - 7 core keys (`Storage`/`Parser`/`Extractor`/`Indexer`/`Resolver`/
//!   `Query`/`Trace`) MUST resolve under default features.
//! - `DaemonKey` MUST resolve when the `daemon` feature is on.
//! - `EmbedKey` MUST resolve when the `embed` feature is on.
//!
//! When a feature is off, the corresponding key type is `cfg`-gated out of
//! the public API, so absence is enforced at compile time (the
//! `--no-default-features` build itself is the proof — no runtime test
//! needed).

use std::path::PathBuf;

use codenexus::kit::{
    build_kit, KitBootstrapConfig, ExtractorKey, IndexerKey, ParserKey, QueryKey, ResolverKey,
    StorageKey, TraceKey,
};
#[cfg(feature = "daemon")]
use codenexus::kit::DaemonKey;
#[cfg(feature = "embed")]
use codenexus::kit::EmbedKey;

/// In-memory database path — keeps tests hermetic (no on-disk cleanup).
fn memory_db_path() -> PathBuf {
    PathBuf::from(":memory:")
}

#[test]
fn all_core_capabilities_resolvable_through_kit() {
    let config = KitBootstrapConfig::new(memory_db_path());
    let kit = build_kit(&config).expect("build_kit");

    // 7 core capability keys MUST all resolve under default features.
    kit.require::<StorageKey>().expect("require_storage");
    kit.require::<ParserKey>().expect("require_parser");
    kit.require::<ExtractorKey>().expect("require_extractor");
    kit.require::<IndexerKey>().expect("require_indexer");
    kit.require::<ResolverKey>().expect("require_resolver");
    kit.require::<QueryKey>().expect("require_query");
    kit.require::<TraceKey>().expect("require_trace");
}

#[cfg(feature = "daemon")]
#[test]
fn daemon_capability_resolvable_when_feature_on() {
    let config = KitBootstrapConfig::new(memory_db_path());
    let kit = build_kit(&config).expect("build_kit");
    kit.require::<DaemonKey>().expect("require_daemon");
}

#[cfg(feature = "embed")]
#[test]
fn embed_capability_resolvable_when_feature_on() {
    let config = KitBootstrapConfig::new(memory_db_path());
    let kit = build_kit(&config).expect("build_kit");
    kit.require::<EmbedKey>().expect("require_embed");
}

#[test]
fn storage_capability_is_functional() {
    // Smoke-test that the resolved Storage capability is a usable facade,
    // not just a type-erased stub. A fresh in-memory Kit has zero projects.
    let config = KitBootstrapConfig::new(memory_db_path());
    let kit = build_kit(&config).expect("build_kit");
    let storage = kit.require::<StorageKey>().expect("require_storage");
    let projects = storage.list_projects().expect("list_projects");
    assert!(
        projects.is_empty(),
        "fresh Kit should have no projects, got {projects:?}"
    );
}
