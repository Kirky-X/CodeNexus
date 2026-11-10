// Copyright (c) 2026 Kirky.X🌠
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
//! with the Kit capability pattern (惯例优先于新颖) and works in
//! both production (via `kit.require::<StorageModule>`) and tests.
pub mod architecture;
pub mod dead_code;

#[cfg(feature = "api-review")]
pub mod api_review;

#[cfg(feature = "community")]
pub mod community;

#[cfg(feature = "analysis")]
pub mod cross_service;

#[cfg(feature = "analysis")]
pub mod rules;

#[cfg(feature = "complexity")]
pub mod complexity;

// Shared path-traversal-protected source reads for analyzers that fall back
// to disk when the indexed `content` column is empty.
#[cfg(any(feature = "complexity", feature = "analysis"))]
// source_io 提升为 crate 顶层模块（`crate::source_io`，无 feature 门控）：
// `service::source` 在 minimal 组合下也要切片读源码，而本模块受
// `feature = "analysis"` 门控。此处整模块 re-export 保持既有调用方路径
// （`analysis::source_io::*` 与 `super::source_io::*`）不变。
pub(crate) use crate::source_io;

// Re-export EdgeType for analysis modules needing multi-edge-type queries.
pub use crate::model::EdgeType;
