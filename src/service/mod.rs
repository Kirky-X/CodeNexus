// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Unified service layer for CLI and MCP command handlers.
//!
//! Each command module defines:
//! - A `core` async function with shared business logic
//! - A CLI wrapper (`#[service_api(cli = true)]`) that calls core + prints to stdout
//! - An MCP wrapper (`#[service_api(mcp = true)]`) that calls core + returns the value
//!
//! Kit injection uses the global [`runtime::kit()`] accessor (OnceLock pattern)
//! because sdforge's `#[service_api]` macro generates standalone functions
//! that cannot accept injected state.

pub mod error;
pub mod query;
pub mod runtime;

pub use runtime::{init_kit, kit};
