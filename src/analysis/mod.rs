// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Code analysis subsystem.
//!
//! Provides graph-based analysis capabilities that aggregate over the existing
//! LadybugDB graph via Cypher queries:
//! - [`dead_code::DeadCodeDetector`] — identifies zero-indegree CALLS functions
//! - [`architecture::ArchitectureAnalyzer`] — produces a project overview
//!
//! All analyzers take a `&dyn Storage` (the trait-kit capability) rather than
//! a `&StorageConnection` directly, matching the codebase convention used by
//! every other CLI command (see `impact_cmd`, `query_cmd`). The specmark
//! design wrote `&'a StorageConnection`, but the Kit only exposes
//! `Arc<dyn Storage>`; using `&dyn Storage` keeps the analyzer consistent
//! with the Kit capability pattern (Rule 11: 惯例优先于新颖) and works in
//! both production (via `kit.require::<StorageModule>()`) and tests.

pub mod architecture;
pub mod dead_code;

#[cfg(feature = "api-review")]
pub mod api_review;

#[cfg(feature = "community")]
pub mod community;

#[cfg(feature = "analysis")]
pub mod cross_service;

#[cfg(feature = "complexity")]
pub mod complexity;

// Re-export EdgeType for analysis modules needing multi-edge-type queries.
pub use crate::model::EdgeType;
