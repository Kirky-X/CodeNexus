// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Memory budget for the indexing pipeline (L1 of the memory-overflow fix).
//!
//! [`MemoryBudget`] is the single source of truth for "memory as a bounded
//! resource" — the missing design dimension identified by the 5-Whys root
//! cause analysis. Every pipeline phase that grows an unbounded collection
//! (`Graph`, `Vec<Node>`, CSV `String`, `RamFirstSources`) MUST consult this
//! budget before growing further, and shed load (flush / batch / degrade)
//! when [`MemoryBudget::check_pressure`] returns [`Pressure::Yellow`] or
//! [`Pressure::Red`].
//!
//! # Design
//!
//! - [`MemoryBudget::from_system`] probes available memory via `sysinfo` and
//!   sets `max_rss_bytes` to 50 % of available, so the indexer never claims
//!   more than half of free RAM.
//! - [`Pressure`] is a three-level signal (`Green` < 60 %, `Yellow` 60-80 %,
//!   `Red` ≥ 80 %) so callers can choose proportionate action:
//!   - `Green` → continue growing.
//!   - `Yellow` → flush current batch, then continue.
//!   - `Red` → abort RAM-first mode, switch to streaming disk-read.
//! - Per-collection soft limit (`per_collection_soft_limit`) and per-cache-entry
//!   limit (`cache_entry_max_bytes`) provide local caps independent of the
//!   global RSS reading, so a single pathological collection cannot OOM the
//!   process before the next RSS poll.
//!
//! # Cross-platform
//!
//! `sysinfo` abstracts Linux `/proc/meminfo`, macOS `host_statistics` and
//! Windows `GlobalMemoryStatusEx` behind a single API; no platform-specific
//! code is used here (Rule 11).

use serde::{Deserialize, Serialize};

/// Green/Yellow threshold: 60 % of `max_rss_bytes`.
const GREEN_YELLOW_RATIO: f64 = 0.60;
/// Yellow/Red threshold: 80 % of `max_rss_bytes`.
const YELLOW_RED_RATIO: f64 = 0.80;
/// Fraction of available memory claimed by `from_system` as the RSS cap.
const FROM_SYSTEM_AVAIL_FRACTION: f64 = 0.50;

/// Three-level memory-pressure signal returned by [`MemoryBudget::check_pressure`].
///
/// Callers choose a proportionate action:
/// - [`Pressure::Green`] — continue growing the collection.
/// - [`Pressure::Yellow`] — flush the current batch, then continue.
/// - [`Pressure::Red`] — abort RAM-first mode, switch to streaming disk-read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pressure {
    /// RSS < 60 % of `max_rss_bytes`. Safe to continue growing.
    Green,
    /// 60 % ≤ RSS < 80 %. Shed load by flushing batches before growing further.
    Yellow,
    /// RSS ≥ 80 %. Aggressively degrade (drop caches, switch to disk-read).
    Red,
}

impl Pressure {
    /// Returns `true` when the caller must immediately shed load.
    #[must_use]
    pub fn is_red(self) -> bool {
        matches!(self, Pressure::Red)
    }

    /// Returns `true` when the caller should flush before growing further
    /// (i.e. `Yellow` or `Red`).
    #[must_use]
    pub fn is_yellow_or_red(self) -> bool {
        !matches!(self, Pressure::Green)
    }
}

/// Memory budget for the indexing pipeline.
///
/// Holds four independent caps:
/// 1. `max_rss_bytes` — process-level hard cap on resident set size.
/// 2. `per_collection_soft_limit` — per-collection size (bytes) above which
///    the collection must be flushed or split into batches.
/// 3. `flush_batch_size` — number of items per batch when flushing.
/// 4. `cache_entry_max_bytes` — max bytes per cache entry; larger entries
///    are rejected by [`crate::cache::CacheStore`] implementations.
///
/// All fields are `pub` so callers can construct custom budgets (e.g. for
/// tests), but the [`with_*`](Self::with_soft_limit) builder methods are
/// preferred for non-test code.
#[derive(Debug, Clone)]
pub struct MemoryBudget {
    /// Process-level hard cap on RSS, in bytes.
    pub max_rss_bytes: u64,
    /// Per-collection soft limit, in bytes.
    pub per_collection_soft_limit: u64,
    /// Number of items per batch when flushing a large collection.
    pub flush_batch_size: usize,
    /// Max bytes per cache entry. Entries larger than this are rejected.
    pub cache_entry_max_bytes: usize,
}

impl MemoryBudget {
    /// Default per-collection soft limit: 256 MiB.
    pub const DEFAULT_SOFT_LIMIT: u64 = 256 * 1024 * 1024;
    /// Default batch size when flushing.
    pub const DEFAULT_FLUSH_BATCH: usize = 5_000;
    /// Default max bytes per cache entry: 64 KiB.
    pub const DEFAULT_CACHE_ENTRY_MAX: usize = 64 * 1024;

    /// Builds a budget by probing the current system's available memory.
    ///
    /// Sets `max_rss_bytes` to 50 % of available memory (so the indexer
    /// never claims more than half of free RAM), and the other fields to
    /// the `DEFAULT_*` constants.
    ///
    /// If `sysinfo` cannot determine available memory (rare; only on some
    /// sandboxed containers), falls back to a conservative 1 GiB RSS cap
    /// so the pipeline still runs but with stricter limits.
    #[must_use]
    pub fn from_system() -> Self {
        let avail = Self::probe_available_memory();
        let max_rss = if avail > 0 {
            ((avail as f64) * FROM_SYSTEM_AVAIL_FRACTION) as u64
        } else {
            // Conservative fallback when sysinfo reports 0 (e.g. some
            // containers restrict /proc/meminfo reads).
            1024 * 1024 * 1024 // 1 GiB
        };
        Self {
            max_rss_bytes: max_rss,
            per_collection_soft_limit: Self::DEFAULT_SOFT_LIMIT,
            flush_batch_size: Self::DEFAULT_FLUSH_BATCH,
            cache_entry_max_bytes: Self::DEFAULT_CACHE_ENTRY_MAX,
        }
    }

    /// Builds a budget with a specific `max_rss_bytes` and default other
    /// fields. Use this in tests instead of [`from_system`](Self::from_system)
    /// to avoid environment-dependent behaviour.
    #[must_use]
    pub fn new(max_rss_bytes: u64) -> Self {
        Self {
            max_rss_bytes,
            per_collection_soft_limit: Self::DEFAULT_SOFT_LIMIT,
            flush_batch_size: Self::DEFAULT_FLUSH_BATCH,
            cache_entry_max_bytes: Self::DEFAULT_CACHE_ENTRY_MAX,
        }
    }

    /// Overrides the per-collection soft limit.
    #[must_use]
    pub fn with_soft_limit(mut self, bytes: u64) -> Self {
        self.per_collection_soft_limit = bytes;
        self
    }

    /// Overrides the batch size used when flushing.
    #[must_use]
    pub fn with_flush_batch_size(mut self, size: usize) -> Self {
        self.flush_batch_size = size;
        self
    }

    /// Overrides the max bytes per cache entry.
    #[must_use]
    pub fn with_cache_entry_max(mut self, bytes: usize) -> Self {
        self.cache_entry_max_bytes = bytes;
        self
    }

    /// Classifies the current RSS reading into a [`Pressure`] level.
    ///
    /// - `current_rss < 60 % of max_rss_bytes` → [`Pressure::Green`]
    /// - `60 % ≤ current_rss < 80 %` → [`Pressure::Yellow`]
    /// - `current_rss ≥ 80 %` → [`Pressure::Red`]
    ///
    /// When `max_rss_bytes == 0` (defensive), always returns [`Pressure::Red`]
    /// so callers cannot bypass the check by setting a zero cap.
    #[must_use]
    pub fn check_pressure(&self, current_rss: u64) -> Pressure {
        if self.max_rss_bytes == 0 {
            return Pressure::Red;
        }
        let ratio = (current_rss as f64) / (self.max_rss_bytes as f64);
        if ratio >= YELLOW_RED_RATIO {
            Pressure::Red
        } else if ratio >= GREEN_YELLOW_RATIO {
            Pressure::Yellow
        } else {
            Pressure::Green
        }
    }

    /// Returns `true` if a collection of `size` bytes exceeds the
    /// per-collection soft limit.
    ///
    /// Callers use this to decide whether to flush a `Vec<Node>` / `String`
    /// / `HashMap` before adding more items.
    #[must_use]
    pub fn collection_exceeds_limit(&self, size: u64) -> bool {
        size >= self.per_collection_soft_limit
    }

    /// Returns `true` if a cache entry of `entry_size` bytes exceeds the
    /// per-entry cap.
    ///
    /// [`crate::cache::CacheStore`] implementations MUST consult this before
    /// calling `set`, and reject oversized entries with a `warn!` log.
    #[must_use]
    pub fn cache_entry_exceeds_limit(&self, entry_size: usize) -> bool {
        entry_size > self.cache_entry_max_bytes
    }

    /// Probes the system's currently available memory, in bytes.
    ///
    /// Returns 0 when the value cannot be determined (e.g. some containers
    /// restrict `/proc/meminfo` reads). The caller is expected to fall back
    /// to a conservative default in that case (see [`from_system`](Self::from_system)).
    #[must_use]
    pub fn probe_available_memory() -> u64 {
        // sysinfo's `System::new()` does NOT populate fields; `refresh_memory()`
        // is required before `available_memory()` returns a real value.
        // The `system` feature is the only one we need (no disk / component /
        // network / user probes).
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        sys.available_memory()
    }
}

impl Default for MemoryBudget {
    /// Defaults to [`from_system`](Self::from_system) so that
    /// `MemoryBudget::default()` produces a sensible environment-aware
    /// budget without an explicit constructor call.
    fn default() -> Self {
        Self::from_system()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Pressure enum ---

    #[test]
    fn pressure_is_red_returns_true_only_for_red() {
        assert!(!Pressure::Green.is_red());
        assert!(!Pressure::Yellow.is_red());
        assert!(Pressure::Red.is_red());
    }

    #[test]
    fn pressure_is_yellow_or_red_returns_true_for_yellow_and_red() {
        assert!(!Pressure::Green.is_yellow_or_red());
        assert!(Pressure::Yellow.is_yellow_or_red());
        assert!(Pressure::Red.is_yellow_or_red());
    }

    #[test]
    fn pressure_is_send_sync() {
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<Pressure>();
    }

    #[test]
    fn pressure_serde_roundtrip() {
        for p in [Pressure::Green, Pressure::Yellow, Pressure::Red] {
            let json = serde_json::to_string(&p).expect("serialize");
            let back: Pressure = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(p, back, "roundtrip failed for {p:?}");
        }
    }

    // --- MemoryBudget defaults ---

    #[test]
    fn default_soft_limit_is_256mib() {
        assert_eq!(MemoryBudget::DEFAULT_SOFT_LIMIT, 256 * 1024 * 1024);
    }

    #[test]
    fn default_flush_batch_is_5000() {
        assert_eq!(MemoryBudget::DEFAULT_FLUSH_BATCH, 5_000);
    }

    #[test]
    fn default_cache_entry_max_is_64kib() {
        assert_eq!(MemoryBudget::DEFAULT_CACHE_ENTRY_MAX, 64 * 1024);
    }

    // --- MemoryBudget::new ---

    #[test]
    fn new_sets_max_rss_and_defaults() {
        let b = MemoryBudget::new(2 * 1024 * 1024 * 1024); // 2 GiB
        assert_eq!(b.max_rss_bytes, 2 * 1024 * 1024 * 1024);
        assert_eq!(
            b.per_collection_soft_limit,
            MemoryBudget::DEFAULT_SOFT_LIMIT
        );
        assert_eq!(b.flush_batch_size, MemoryBudget::DEFAULT_FLUSH_BATCH);
        assert_eq!(
            b.cache_entry_max_bytes,
            MemoryBudget::DEFAULT_CACHE_ENTRY_MAX
        );
    }

    #[test]
    fn new_with_zero_max_rss_is_allowed_but_always_red() {
        // Defensive: a zero cap should NOT bypass the check; it should
        // force Red so callers cannot silently disable the budget.
        let b = MemoryBudget::new(0);
        assert_eq!(b.check_pressure(0), Pressure::Red);
        assert_eq!(b.check_pressure(1), Pressure::Red);
    }

    // --- Builder methods ---

    #[test]
    fn with_soft_limit_overrides_default() {
        let b = MemoryBudget::new(1024).with_soft_limit(512 * 1024 * 1024);
        assert_eq!(b.per_collection_soft_limit, 512 * 1024 * 1024);
    }

    #[test]
    fn with_flush_batch_size_overrides_default() {
        let b = MemoryBudget::new(1024).with_flush_batch_size(1_000);
        assert_eq!(b.flush_batch_size, 1_000);
    }

    #[test]
    fn with_cache_entry_max_overrides_default() {
        let b = MemoryBudget::new(1024).with_cache_entry_max(128 * 1024);
        assert_eq!(b.cache_entry_max_bytes, 128 * 1024);
    }

    #[test]
    fn builder_methods_chain() {
        let b = MemoryBudget::new(1024)
            .with_soft_limit(100)
            .with_flush_batch_size(200)
            .with_cache_entry_max(300);
        assert_eq!(b.per_collection_soft_limit, 100);
        assert_eq!(b.flush_batch_size, 200);
        assert_eq!(b.cache_entry_max_bytes, 300);
    }

    // --- check_pressure boundaries ---

    #[test]
    fn check_pressure_zero_rss_is_green() {
        let b = MemoryBudget::new(1_000_000);
        assert_eq!(b.check_pressure(0), Pressure::Green);
    }

    #[test]
    fn check_pressure_just_below_yellow_is_green() {
        let b = MemoryBudget::new(1_000_000);
        // 60 % of 1_000_000 = 600_000; one byte below is Green.
        assert_eq!(b.check_pressure(599_999), Pressure::Green);
    }

    #[test]
    fn check_pressure_at_60_percent_is_yellow() {
        let b = MemoryBudget::new(1_000_000);
        assert_eq!(b.check_pressure(600_000), Pressure::Yellow);
    }

    #[test]
    fn check_pressure_just_below_red_is_yellow() {
        let b = MemoryBudget::new(1_000_000);
        // 80 % of 1_000_000 = 800_000; one byte below is Yellow.
        assert_eq!(b.check_pressure(799_999), Pressure::Yellow);
    }

    #[test]
    fn check_pressure_at_80_percent_is_red() {
        let b = MemoryBudget::new(1_000_000);
        assert_eq!(b.check_pressure(800_000), Pressure::Red);
    }

    #[test]
    fn check_pressure_at_full_cap_is_red() {
        let b = MemoryBudget::new(1_000_000);
        assert_eq!(b.check_pressure(1_000_000), Pressure::Red);
    }

    #[test]
    fn check_pressure_above_cap_is_red() {
        let b = MemoryBudget::new(1_000_000);
        assert_eq!(b.check_pressure(2_000_000), Pressure::Red);
    }

    // --- collection_exceeds_limit ---

    #[test]
    fn collection_exceeds_limit_below_limit_returns_false() {
        let b = MemoryBudget::new(1024).with_soft_limit(1_000);
        assert!(!b.collection_exceeds_limit(999));
    }

    #[test]
    fn collection_exceeds_limit_at_limit_returns_true() {
        // `>=` so that "at limit" triggers flush (defensive).
        let b = MemoryBudget::new(1024).with_soft_limit(1_000);
        assert!(b.collection_exceeds_limit(1_000));
    }

    #[test]
    fn collection_exceeds_limit_above_limit_returns_true() {
        let b = MemoryBudget::new(1024).with_soft_limit(1_000);
        assert!(b.collection_exceeds_limit(1_001));
    }

    // --- cache_entry_exceeds_limit ---

    #[test]
    fn cache_entry_exceeds_limit_below_max_returns_false() {
        let b = MemoryBudget::new(1024).with_cache_entry_max(1_000);
        assert!(!b.cache_entry_exceeds_limit(999));
    }

    #[test]
    fn cache_entry_exceeds_limit_at_max_returns_false() {
        // `>` (strict) so that "at max" is allowed (an entry exactly at the
        // cap is acceptable; only larger entries are rejected).
        let b = MemoryBudget::new(1024).with_cache_entry_max(1_000);
        assert!(!b.cache_entry_exceeds_limit(1_000));
    }

    #[test]
    fn cache_entry_exceeds_limit_above_max_returns_true() {
        let b = MemoryBudget::new(1024).with_cache_entry_max(1_000);
        assert!(b.cache_entry_exceeds_limit(1_001));
    }

    // --- from_system (env-aware, but assert invariants) ---

    #[test]
    fn from_system_returns_nonzero_max_rss() {
        // On any reasonable test host, available memory is non-zero, so
        // from_system must produce a non-zero cap. If this fails, the test
        // host is in a pathological sandbox — investigate before ignoring.
        let b = MemoryBudget::from_system();
        assert!(b.max_rss_bytes > 0, "from_system returned zero max_rss");
    }

    #[test]
    fn from_system_applies_other_defaults() {
        let b = MemoryBudget::from_system();
        assert_eq!(
            b.per_collection_soft_limit,
            MemoryBudget::DEFAULT_SOFT_LIMIT
        );
        assert_eq!(b.flush_batch_size, MemoryBudget::DEFAULT_FLUSH_BATCH);
        assert_eq!(
            b.cache_entry_max_bytes,
            MemoryBudget::DEFAULT_CACHE_ENTRY_MAX
        );
    }

    #[test]
    fn default_equals_from_system_invariants() {
        let d = MemoryBudget::default();
        assert!(d.max_rss_bytes > 0);
        assert_eq!(
            d.per_collection_soft_limit,
            MemoryBudget::DEFAULT_SOFT_LIMIT
        );
    }

    // --- probe_available_memory ---

    #[test]
    fn probe_available_memory_returns_some_value_on_normal_host() {
        // On CI / dev hosts this should be > 0. We do not assert a specific
        // value (env-dependent), only that the probe did not return 0.
        let avail = MemoryBudget::probe_available_memory();
        assert!(avail > 0, "probe_available_memory returned 0 — sandbox?");
    }

    // --- Send + Sync ---

    #[test]
    fn memory_budget_is_send_sync() {
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<MemoryBudget>();
    }

    #[test]
    fn memory_budget_is_clone() {
        let b = MemoryBudget::new(1024);
        let _b2 = b.clone();
    }

    // --- Edge cases ---

    #[test]
    fn very_large_max_rss_does_not_overflow() {
        // u64::MAX must not panic; ratio computation uses f64.
        let b = MemoryBudget::new(u64::MAX);
        assert_eq!(b.check_pressure(0), Pressure::Green);
        // u64::MAX * 0.6 as f64 is still < 1.0 ratio boundary; check that
        // calling with u64::MAX itself classifies as Red.
        assert_eq!(b.check_pressure(u64::MAX), Pressure::Red);
    }

    #[test]
    fn check_pressure_with_one_byte_max_rss_classifies_correctly() {
        // 1-byte cap: any non-zero RSS is ≥ 80 % → Red.
        let b = MemoryBudget::new(1);
        assert_eq!(b.check_pressure(0), Pressure::Green);
        assert_eq!(b.check_pressure(1), Pressure::Red);
    }
}
