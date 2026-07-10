// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! CLI error types.
//!
//! # Exit codes (PRD §4.1.6)
//!
//! | Code | Meaning               | Variants                                   |
//! |------|-----------------------|--------------------------------------------|
//! | 0    | success               | —                                          |
//! | 1    | internal/system error | Internal, Io, Kit, Json, Daemon            |
//! | 2    | client error          | InvalidInput, ProjectNotFound, Query, Trace, Storage |
//! | 3    | (reserved)            | —                                          |
//! | 4    | not found / corrupt   | NotFound, Index corrupt, Kit corrupt       |
//!
//! See [`error::CliError`] for the full mapping.

pub mod error;

pub use error::{CliError, Result};
