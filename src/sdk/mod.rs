// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! `Nexus` — the SDK facade over the assembled capability kit.
//!
//! One type owns the built kit and exposes the capability traits
//! (`Arc<dyn Storage>` …) without leaking any trait-kit type through
//! signatures. Construction funnels through [`Nexus::open`] (full config)
//! or [`Nexus::from_kit`] (a kit already assembled); the process-global
//! path is [`Nexus::from_runtime`].
//!
//! Internal shape for now (`#[doc(hidden)]` in `lib.rs`): the public
//! commitment is deferred until trait-kit 0.5.0 final (design D-2).

pub mod nexus;

pub use nexus::{Nexus, NexusError};
