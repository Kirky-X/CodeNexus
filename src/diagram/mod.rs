// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Deterministic architecture-diagram pipeline (absorbed from archify).
//!
//! Typed JSON IR → deterministic grid layout → orthogonal routing →
//! reproducible SVG → geometric quality gates → self-contained interactive
//! HTML → atomic delivery with a SHA-256 receipt.
//!
//! Design rules absorbed from archify's engineering discipline:
//! - **Determinism**: identical IR renders byte-identical output.
//! - **Fail-closed**: a failing quality gate never replaces the last-good
//!   artifact (`deliver` stages into the target directory and renames).
//! - **Repair receipts**: every gate failure is a stable rule code with
//!   measured evidence and curated fixes — never a bare message.
//! - **Truth boundary**: edges come from the static index (CALLS, HTTP
//!   facts); the diagram never claims runtime behavior.

pub mod deliver;
pub mod ir;
pub mod layout;
pub mod quality;
pub mod render;
pub mod route;
pub mod svg;
pub mod text;
