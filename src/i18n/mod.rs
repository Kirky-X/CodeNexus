// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Message-level internationalization (Fluent + locale detection chain).
//!
//! Distinct from [`crate::model::i18n`], which is Unicode *text processing*
//! (case folding / NFC normalization for tokenization) — this module
//! localizes user-facing messages (daemon logs, server error responses).
//!
//! Provides:
//! - **Locale detection**: automatic detection with the forced chain
//!   `CODENEXUS_LANG` → `LC_ALL` → `LC_MESSAGES` → `LANG` → `sys-locale`
//!   → `"en"`; the resolved domain is exactly `{en, zh}` (see
//!   [`locale`] and change `unify-rust-i18n` reference-pattern §2).
//! - **Message catalog**: Fluent (FTL) resources embedded at compile time
//!   for two languages (en, zh) and registered into the sdforge i18n
//!   registry by [`init`] (see [`catalog`]).
//!
//! Only `en`/`zh` are supported; every fallback terminates in `en`.
//! [`t`] never panics: before [`init`] (or on any lookup miss) it returns
//! the key itself — bare keys must never crash a daemon log line or an
//! HTTP response. Note that translations are only available after
//! [`init`] runs (application entry points must call it).

mod catalog;
mod locale;

pub use catalog::{init, t, tr};
pub use locale::{current_locale, set_locale};
