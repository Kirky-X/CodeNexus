// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! LSP `textDocument/references` result cache (C9, R-lsp-004).
//!
//! Caches `Vec<lsp_types::Location>` results keyed by `(uri, line, column)`
//! for up to `TTL` (default 5 minutes). Bounds memory with an LRU policy
//! (default 1000 entries). Time is injectable via the [`Clock`] trait so
//! tests can fast-forward without `thread::sleep`.
//!
//! # Design
//!
//! - **Lazy TTL expiration**: expired entries are evicted on read rather
//!   than via a background sweeper — keeps the cache lock-free of timers
//!   and avoids a dedicated reaper thread.
//! - **LRU via `VecDeque`**: O(n) `retain` on eviction is acceptable
//!   because `n ≤ capacity` (default 1000) and evictions are infrequent
//!   relative to `get`/`insert` on hot keys.
//! - **Mock clock injection**: [`ReferencesCache::with_clock`] accepts any
//!   `Arc<dyn Clock>`, enabling deterministic TTL tests in milliseconds.
//!
//! # Feature gating
//!
//! Compiles only under the `lsp` cargo feature (same as the rest of
//! `crate::lsp`).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lsp_types::Location;

/// Default cache TTL: 5 minutes (specmark `specs/lsp/spec.md` R-lsp-004).
pub const DEFAULT_TTL: Duration = Duration::from_secs(300);

/// Default LRU capacity (specmark `specs/lsp/spec.md` R-lsp-004).
pub const DEFAULT_CAPACITY: usize = 1_000;

/// Abstract time source so tests can fast-forward without sleeping.
///
/// Production code uses [`SystemClock`]; tests inject [`MockClock`].
pub trait Clock: Send + Sync {
    /// Returns the current instant.
    fn now(&self) -> Instant;
}

/// Real-time clock backed by [`Instant::now`].
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Deterministic clock for tests — advances only when [`MockClock::advance`]
/// is called.
#[derive(Debug)]
pub struct MockClock {
    inner: Mutex<Instant>,
}

impl MockClock {
    /// Creates a mock clock anchored at `Instant::now()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Instant::now()),
        }
    }

    /// Creates a mock clock anchored at a specific `Instant` (useful for
    /// reproducible tests that need a fixed epoch).
    #[must_use]
    pub fn with_start(start: Instant) -> Self {
        Self {
            inner: Mutex::new(start),
        }
    }

    /// Advances the mock clock forward by `dur`. Panics on overflow
    /// (cannot rewind — LSP cache timestamps are monotonic).
    pub fn advance(&self, dur: Duration) {
        let mut guard = self.inner.lock().expect("MockClock mutex poisoned");
        *guard = guard
            .checked_add(dur)
            .expect("MockClock::advance overflowed Instant");
    }
}

impl Default for MockClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for MockClock {
    fn now(&self) -> Instant {
        *self.inner.lock().expect("MockClock mutex poisoned")
    }
}

/// Cache key: `(uri, line, column)` — the triple specmark R-lsp-004 mandates.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    /// File URI (`file://...`) exactly as the LSP server sees it.
    pub uri: String,
    /// 0-based line number (LSP `Position.line` convention).
    pub line: u32,
    /// 0-based column / character offset (LSP `Position.character`).
    pub column: u32,
}

impl CacheKey {
    /// Creates a new cache key from the raw `(uri, line, column)` triple.
    #[must_use]
    pub fn new(uri: String, line: u32, column: u32) -> Self {
        Self { uri, line, column }
    }
}

/// LRU + TTL cache for `textDocument/references` results.
///
/// Thread-safe via internal `Mutex`. Capacity-bounded by LRU eviction
/// (oldest-accessed entry removed when capacity exceeded). TTL is checked
/// lazily on read — expired entries are removed and treated as misses.
///
/// # Examples
///
/// ```
/// use codenexus::lsp::references_cache::{CacheKey, ReferencesCache};
/// use lsp_types::{Location, Position, Range, Uri};
/// use std::str::FromStr;
///
/// let cache = ReferencesCache::new();
/// let key = CacheKey::new("file:///tmp/x.rs".to_string(), 5, 10);
/// let loc = Location {
///     uri: Uri::from_str("file:///tmp/x.rs").unwrap(),
///     range: Range {
///         start: Position { line: 5, character: 0 },
///         end: Position { line: 5, character: 10 },
///     },
/// };
/// cache.insert(key.clone(), vec![loc.clone()]);
/// let got = cache.get(&key).expect("entry present");
/// assert_eq!(got.len(), 1);
/// ```
pub struct ReferencesCache {
    inner: Mutex<CacheInner>,
    clock: Arc<dyn Clock>,
    ttl: Duration,
    capacity: usize,
}

struct CacheInner {
    entries: HashMap<CacheKey, (Instant, Vec<Location>)>,
    /// LRU ordering: front = most recently used, back = least recently used.
    lru: VecDeque<CacheKey>,
}

impl ReferencesCache {
    /// Default LRU capacity (1000 entries, per spec R-lsp-004).
    pub const DEFAULT_CAPACITY: usize = DEFAULT_CAPACITY;

    /// Default TTL (5 minutes, per spec R-lsp-004).
    pub const DEFAULT_TTL: Duration = DEFAULT_TTL;

    /// Creates a cache with system clock, default TTL (5 min) and default
    /// capacity (1000).
    #[must_use]
    pub fn new() -> Self {
        Self::with_clock(
            Arc::new(SystemClock),
            Self::DEFAULT_TTL,
            Self::DEFAULT_CAPACITY,
        )
    }

    /// Creates a cache with a custom clock (for tests), TTL, and capacity.
    ///
    /// # Panics
    ///
    /// Panics if `capacity == 0` (a zero-capacity cache can never hold
    /// anything and is almost certainly a caller bug).
    #[must_use]
    pub fn with_clock(clock: Arc<dyn Clock>, ttl: Duration, capacity: usize) -> Self {
        assert!(capacity > 0, "ReferencesCache capacity must be > 0");
        let cap_hint = capacity.min(1024);
        Self {
            inner: Mutex::new(CacheInner {
                entries: HashMap::with_capacity(cap_hint),
                lru: VecDeque::with_capacity(cap_hint),
            }),
            clock,
            ttl,
            capacity,
        }
    }

    /// Looks up `key` in the cache. Returns `Some` only if the entry exists
    /// AND has not expired (age <= TTL). Expired entries are evicted on read
    /// (lazy expiration).
    pub fn get(&self, key: &CacheKey) -> Option<Vec<Location>> {
        let mut guard = self.inner.lock().expect("cache mutex poisoned");
        let now = self.clock.now();
        let ttl = self.ttl;

        let stored_at = match guard.entries.get(key) {
            Some((stored_at, _)) => *stored_at,
            None => return None,
        };

        if now.duration_since(stored_at) > ttl {
            // Lazy expiration: evict on read.
            guard.entries.remove(key);
            guard.lru.retain(|k| k != key);
            return None;
        }

        // Move to front (most recently used).
        guard.lru.retain(|k| k != key);
        guard.lru.push_front(key.clone());

        guard.entries.get(key).map(|(_, locs)| locs.clone())
    }

    /// Inserts `key -> locations` with the current timestamp. Evicts the
    /// least-recently-used entry if at capacity. Overwrites existing value
    /// (and refreshes timestamp + LRU position) if `key` already present.
    pub fn insert(&self, key: CacheKey, locations: Vec<Location>) {
        let mut guard = self.inner.lock().expect("cache mutex poisoned");
        let now = self.clock.now();

        if guard.entries.contains_key(&key) {
            // Refresh: remove from LRU, will re-push front below.
            guard.lru.retain(|k| k != &key);
        } else if guard.entries.len() >= self.capacity {
            // Evict LRU (back of deque).
            if let Some(evicted) = guard.lru.pop_back() {
                guard.entries.remove(&evicted);
            }
        }

        guard.entries.insert(key.clone(), (now, locations));
        guard.lru.push_front(key);
    }

    /// Invalidates all entries for `uri` (called when `textDocument/didChange`
    /// fires for that file). Returns the number of entries evicted.
    pub fn invalidate_uri(&self, uri: &str) -> usize {
        let mut guard = self.inner.lock().expect("cache mutex poisoned");
        let to_remove: Vec<CacheKey> = guard
            .entries
            .keys()
            .filter(|k| k.uri == uri)
            .cloned()
            .collect();
        let count = to_remove.len();
        for key in to_remove {
            guard.entries.remove(&key);
            guard.lru.retain(|k| k != &key);
        }
        count
    }

    /// Returns the current number of entries (including any expired ones
    /// not yet lazily evicted).
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("cache mutex poisoned")
            .entries
            .len()
    }

    /// Returns `true` if the cache currently holds zero entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the configured TTL.
    pub const fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Returns the configured capacity.
    pub const fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Default for ReferencesCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range, Uri};
    use std::str::FromStr;

    fn loc(line: u32) -> Location {
        Location {
            uri: Uri::from_str("file:///tmp/x.rs").unwrap(),
            range: Range {
                start: Position { line, character: 0 },
                end: Position { line, character: 5 },
            },
        }
    }

    #[test]
    fn new_uses_defaults() {
        let c = ReferencesCache::new();
        assert_eq!(c.ttl(), Duration::from_secs(300));
        assert_eq!(c.capacity(), 1_000);
        assert!(c.is_empty());
    }

    #[test]
    fn insert_then_get_returns_value() {
        let c = ReferencesCache::new();
        let key = CacheKey::new("file:///tmp/x.rs".into(), 5, 10);
        c.insert(key.clone(), vec![loc(5)]);
        assert_eq!(c.len(), 1);
        let got = c.get(&key).expect("entry present");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].range.start.line, 5);
    }

    #[test]
    fn get_missing_returns_none() {
        let c = ReferencesCache::new();
        let key = CacheKey::new("file:///tmp/x.rs".into(), 5, 10);
        assert!(c.get(&key).is_none());
    }

    #[test]
    fn expired_entry_is_evicted_on_read() {
        let clock = Arc::new(MockClock::new());
        let c = ReferencesCache::with_clock(clock.clone(), Duration::from_secs(60), 100);
        let key = CacheKey::new("file:///tmp/x.rs".into(), 5, 10);
        c.insert(key.clone(), vec![loc(5)]);
        assert!(c.get(&key).is_some());

        clock.advance(Duration::from_secs(61));
        assert!(c.get(&key).is_none(), "expired entry should be evicted");
        assert_eq!(c.len(), 0, "eviction should remove from cache");
    }

    #[test]
    fn ttl_boundary_exactly_at_ttl_is_hit() {
        // age == ttl is fresh; age > ttl is expired (half-open interval).
        let clock = Arc::new(MockClock::new());
        let c = ReferencesCache::with_clock(clock.clone(), Duration::from_secs(60), 100);
        let key = CacheKey::new("file:///tmp/x.rs".into(), 5, 10);
        c.insert(key.clone(), vec![loc(5)]);
        clock.advance(Duration::from_secs(60));
        assert!(c.get(&key).is_some(), "age == ttl should still be fresh");
    }

    #[test]
    fn lru_eviction_when_capacity_exceeded() {
        let clock = Arc::new(MockClock::new());
        let c = ReferencesCache::with_clock(clock, Duration::from_secs(600), 2);
        let k1 = CacheKey::new("file:///tmp/a.rs".into(), 1, 1);
        let k2 = CacheKey::new("file:///tmp/b.rs".into(), 2, 2);
        let k3 = CacheKey::new("file:///tmp/c.rs".into(), 3, 3);

        c.insert(k1.clone(), vec![loc(1)]);
        c.insert(k2.clone(), vec![loc(2)]);
        // k1 and k2 present; capacity=2. Insert k3 → evict LRU (k1, since k2
        // was just inserted after k1).
        c.insert(k3.clone(), vec![loc(3)]);

        assert!(c.get(&k1).is_none(), "k1 should have been LRU-evicted");
        assert!(c.get(&k2).is_some());
        assert!(c.get(&k3).is_some());
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn lru_refresh_on_get_prevents_eviction() {
        let clock = Arc::new(MockClock::new());
        let c = ReferencesCache::with_clock(clock, Duration::from_secs(600), 2);
        let k1 = CacheKey::new("file:///tmp/a.rs".into(), 1, 1);
        let k2 = CacheKey::new("file:///tmp/b.rs".into(), 2, 2);
        let k3 = CacheKey::new("file:///tmp/c.rs".into(), 3, 3);

        c.insert(k1.clone(), vec![loc(1)]);
        c.insert(k2.clone(), vec![loc(2)]);
        // Access k1 to make it MRU; k2 becomes LRU.
        let _ = c.get(&k1);
        c.insert(k3.clone(), vec![loc(3)]);

        assert!(c.get(&k1).is_some(), "k1 was refreshed, should survive");
        assert!(c.get(&k2).is_none(), "k2 was LRU, should be evicted");
        assert!(c.get(&k3).is_some());
    }

    #[test]
    fn invalidate_uri_removes_all_entries_for_file() {
        let c = ReferencesCache::new();
        c.insert(CacheKey::new("file:///tmp/x.rs".into(), 1, 1), vec![loc(1)]);
        c.insert(
            CacheKey::new("file:///tmp/x.rs".into(), 5, 10),
            vec![loc(5)],
        );
        c.insert(CacheKey::new("file:///tmp/y.rs".into(), 1, 1), vec![loc(1)]);

        let evicted = c.invalidate_uri("file:///tmp/x.rs");
        assert_eq!(evicted, 2);
        assert_eq!(c.len(), 1, "y.rs entry must survive");
    }

    #[test]
    fn invalidate_uri_no_match_returns_zero() {
        let c = ReferencesCache::new();
        c.insert(CacheKey::new("file:///tmp/x.rs".into(), 1, 1), vec![loc(1)]);
        assert_eq!(c.invalidate_uri("file:///tmp/other.rs"), 0);
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn insert_overwrites_existing_key() {
        let c = ReferencesCache::new();
        let key = CacheKey::new("file:///tmp/x.rs".into(), 5, 10);
        c.insert(key.clone(), vec![loc(5)]);
        c.insert(key.clone(), vec![loc(5), loc(6)]);
        assert_eq!(c.len(), 1, "overwrite must not grow cache");
        let got = c.get(&key).expect("entry present");
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn mock_clock_with_start_is_deterministic() {
        let anchor = Instant::now();
        let c = MockClock::with_start(anchor);
        assert_eq!(c.now(), anchor);
        c.advance(Duration::from_secs(10));
        assert_eq!(c.now(), anchor + Duration::from_secs(10));
    }

    #[test]
    #[should_panic(expected = "capacity must be > 0")]
    fn zero_capacity_panics() {
        let clock = Arc::new(SystemClock) as Arc<dyn Clock>;
        let _ = ReferencesCache::with_clock(clock, Duration::from_secs(60), 0);
    }

    #[test]
    fn system_clock_advances_monotonically() {
        let c = SystemClock;
        let t1 = c.now();
        // Spin briefly to ensure `Instant::now` advances past t1.
        while c.now() == t1 {
            std::hint::spin_loop();
        }
        assert!(c.now() > t1);
    }
}
