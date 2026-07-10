// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Intermediate Representation (IR): shared data types.
//!
//! Defines the pure data structures produced by the parse phase and consumed
//! by the resolve phase. Placing these in a dedicated `ir` module breaks the
//! `parse ↔ resolve` bidirectional dependency: both `parse` and `resolve`
//! depend on `ir`, while `parse` additionally depends on `resolve::FqnGenerator`
//! (FQN generation is part of the parse phase per ADD §7.1).
//!
//! # Types
//!
//! - [`ImportInfo`], [`CallInfo`], [`AssignInfo`], [`ExternInfo`],
//!   [`ReadInfo`], [`WriteInfo`]: intermediate extraction records.
//! - [`ExtractResult`]: the per-file extraction output aggregating all records.

pub mod extract_result;
pub mod types;

pub use extract_result::ExtractResult;
pub use types::{AssignInfo, CallInfo, ExternInfo, ImportInfo, ReadInfo, WriteInfo};
