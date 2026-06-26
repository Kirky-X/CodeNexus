// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! File discovery subsystem.
//!
//! Wraps the [`ignore`] crate to walk repositories while honoring
//! `.gitignore`/`.codenexusignore` rules and the `ALWAYS_SKIP_DIRS` allowlist
//! (ADR-012, BR-INDEX-006).

mod error;
mod walker;

pub use error::DiscoverError;
pub use walker::{is_code_file, should_skip_dir, FileInfo, Walker, ALWAYS_SKIP_DIRS};
