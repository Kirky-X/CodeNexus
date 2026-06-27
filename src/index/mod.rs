// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Indexing pipeline (Facade pattern).
//!
//! Orchestrates discover → parse → resolve → storage, computes SHA-256 file
//! hashes for incremental indexing (ADR-009), and exposes [`IndexFacade`] as
//! the single entry point for the indexing workflow.
//!
//! # Modules
//!
//! - [`error`]: [`IndexError`] and [`Result`](error::Result) alias.
//! - [`hash`]: SHA-256 file/content hashing (ADR-009).
//! - [`incremental`]: [`FileDiff`] and [`diff_files`] for incremental indexing
//!   (BR-INDEX-001~003).
//! - [`pipeline`]: [`IndexFacade`] (Facade), [`Pipeline`], [`IndexResult`].

pub mod capability;
pub mod module;
pub mod error;
pub mod hash;
pub mod incremental;
pub mod pipeline;

pub use error::{IndexError, Result};
pub use hash::{compute_content_hash, compute_file_hash};
pub use incremental::{diff_files, FileDiff};
pub use pipeline::{IndexFacade, IndexResult, Pipeline};
pub use module::{IndexConfig, IndexerModule, IndexerModuleBuilder};
