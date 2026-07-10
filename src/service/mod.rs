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

pub mod architecture;
pub mod api_impact;
pub mod clean;
pub mod community;
pub mod complexity;
pub mod context;
pub mod cross_service;
pub mod daemon;
pub mod dead_code;
pub mod detect_changes;
pub mod error;
pub mod export;
pub mod hook;
pub mod impact;
pub mod import;
pub mod index;
pub mod list;
pub mod lsp;
pub mod query;
pub mod rename;
pub mod route_map;
pub mod runtime;
pub mod search;
pub mod setup;
pub mod shape_check;
pub mod status;
pub mod tool_map;
pub mod trace;

pub use runtime::{init_kit, kit};
