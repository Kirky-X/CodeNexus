// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! SHA-256 file content hashing (ADR-009).
//!
//! Provides deterministic SHA-256 digests used by the incremental indexer to
//! detect file changes (BR-INDEX-001~003). Hashes are returned as lowercase
//! hexadecimal strings (64 characters), matching the format stored in the
//! `File` node's `hash` property.

use std::path::Path;

#[cfg(feature = "cache")]
use crate::cache::CacheStore;
use sha2::{Digest, Sha256};

/// Computes the SHA-256 hash of the file at `path`, returning the digest as
/// a lowercase 64-character hex string.
///
/// # Errors
///
/// Returns [`std::io::Error`] if the file cannot be read.
pub fn compute_file_hash(path: &Path) -> Result<String, std::io::Error> {
    let content = std::fs::read(path)?;
    Ok(compute_content_hash(&content))
}

/// Computes the SHA-256 hash of the file at `path` with optional cache.
///
/// When `cache` is `Some`, queries the cache first using a key derived from
/// the file path **and** its mtime (nanoseconds since `UNIX_EPOCH`). On a
/// miss, the hash is computed via [`compute_file_hash`] and stored in the
/// cache (raw UTF-8 bytes of the hex string).
///
/// # Mtime-based invalidation
///
/// The cache key embeds the file mtime, so any file modification that
/// bumps mtime (a write, truncate, or explicit `set_modified`) automatically
/// produces a different key — the stale entry becomes unreachable and the
/// new computation overwrites it. If mtime cannot be read (e.g., the
/// filesystem does not support it), the function silently falls back to
/// uncached [`compute_file_hash`].
///
/// # Errors
///
/// Returns [`std::io::Error`] if the file cannot be read.
#[cfg(feature = "cache")]
pub fn compute_file_hash_cached(
    path: &Path,
    cache: Option<&dyn CacheStore>,
) -> Result<String, std::io::Error> {
    let cache = match cache {
        Some(c) => c,
        None => return compute_file_hash(path),
    };

    // Build cache key (path + mtime). Fall back to uncached on failure.
    let key = match build_cache_key(path) {
        Some(k) => k,
        None => return compute_file_hash(path),
    };

    // Cache hit: parse UTF-8 string from bytes. Fall through on parse
    // failure (corrupt entry) to recompute and overwrite.
    if let Some(cached) = cache.get(&key) {
        if let Ok(s) = String::from_utf8(cached) {
            return Ok(s);
        }
    }

    // Cache miss: compute, store, return.
    let hash = compute_file_hash(path)?;
    cache.set(&key, hash.as_bytes().to_vec());
    Ok(hash)
}

/// Builds the cache key for a file hash entry:
/// `hash:file:{path}:{mtime_nanos}`.
///
/// Returns `None` if either `metadata` or `modified` fails — the caller
/// should fall back to uncached computation in that case.
#[cfg(feature = "cache")]
fn build_cache_key(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    let mtime = metadata.modified().ok()?;
    let nanos = mtime
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(format!("hash:file:{}:{}", path.display(), nanos))
}

/// Computes the SHA-256 hash of `content`, returning the digest as a
/// lowercase 64-character hex string.
#[must_use]
pub fn compute_content_hash(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    let digest = hasher.finalize();
    // 32 bytes → 64 hex chars, lowercase.
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::NamedTempFile;

    // --- compute_content_hash ---

    #[test]
    fn compute_content_hash_returns_64_char_hex_string() {
        let hash = compute_content_hash(b"hello world");
        assert_eq!(hash.len(), 64, "SHA-256 hex digest must be 64 chars");
        assert!(
            hash.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "hash must be lowercase hex: {hash}"
        );
    }

    #[test]
    fn compute_content_hash_known_value() {
        // SHA-256("hello world") = b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
        let hash = compute_content_hash(b"hello world");
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn compute_content_hash_empty_input() {
        // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let hash = compute_content_hash(b"");
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn compute_content_hash_same_content_same_hash() {
        let a = compute_content_hash(b"fn main() {}");
        let b = compute_content_hash(b"fn main() {}");
        assert_eq!(a, b, "identical content must produce identical hashes");
    }

    #[test]
    fn compute_content_hash_different_content_different_hash() {
        let a = compute_content_hash(b"fn main() {}");
        let b = compute_content_hash(b"fn main() { }");
        assert_ne!(a, b, "different content must produce different hashes");
    }

    #[test]
    fn compute_content_hash_handles_binary_content() {
        let binary: Vec<u8> = (0..=255u8).collect();
        let hash = compute_content_hash(&binary);
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn compute_content_hash_handles_unicode() {
        let hash = compute_content_hash("fn 你好() {}".as_bytes());
        assert_eq!(hash.len(), 64);
    }

    // --- compute_file_hash ---

    #[test]
    fn compute_file_hash_returns_64_char_hex_string() {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"fn main() {}").unwrap();
        let hash = compute_file_hash(tmp.path()).expect("hash should succeed");
        assert_eq!(hash.len(), 64);
        assert!(
            hash.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "hash must be lowercase hex: {hash}"
        );
    }

    #[test]
    fn compute_file_hash_matches_content_hash() {
        let tmp = NamedTempFile::new().unwrap();
        let content = b"int main(void) { return 0; }";
        fs::write(tmp.path(), content).unwrap();
        let file_hash = compute_file_hash(tmp.path()).unwrap();
        let content_hash = compute_content_hash(content);
        assert_eq!(file_hash, content_hash);
    }

    #[test]
    fn compute_file_hash_same_file_same_hash() {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"fn foo() {}").unwrap();
        let a = compute_file_hash(tmp.path()).unwrap();
        let b = compute_file_hash(tmp.path()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn compute_file_hash_changes_when_content_changes() {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"fn foo() {}").unwrap();
        let before = compute_file_hash(tmp.path()).unwrap();
        fs::write(tmp.path(), b"fn bar() {}").unwrap();
        let after = compute_file_hash(tmp.path()).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn compute_file_hash_nonexistent_file_returns_error() {
        let result = compute_file_hash(Path::new("/nonexistent/path/does_not_exist.rs"));
        assert!(result.is_err(), "nonexistent file should error");
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn compute_file_hash_empty_file() {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"").unwrap();
        let hash = compute_file_hash(tmp.path()).unwrap();
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}

#[cfg(feature = "cache")]
mod cached_tests {
    use super::*;
    use crate::cache::CacheStore;
    use std::collections::HashMap;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tempfile::NamedTempFile;

    /// Mock `CacheStore` for testing — counts get/set calls and stores entries
    /// in an in-memory `HashMap`.
    struct CountingCache {
        gets: AtomicUsize,
        sets: AtomicUsize,
        inner: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl CountingCache {
        fn new() -> Self {
            Self {
                gets: AtomicUsize::new(0),
                sets: AtomicUsize::new(0),
                inner: Mutex::new(HashMap::new()),
            }
        }

        fn gets(&self) -> usize {
            self.gets.load(Ordering::SeqCst)
        }

        fn sets(&self) -> usize {
            self.sets.load(Ordering::SeqCst)
        }

        /// Returns a snapshot of all cached entries.
        fn snapshot(&self) -> HashMap<String, Vec<u8>> {
            self.inner.lock().expect("lock").clone()
        }
    }

    impl CacheStore for CountingCache {
        fn get(&self, key: &str) -> Option<Vec<u8>> {
            self.gets.fetch_add(1, Ordering::SeqCst);
            self.inner.lock().expect("lock").get(key).cloned()
        }

        fn set(&self, key: &str, val: Vec<u8>) {
            self.sets.fetch_add(1, Ordering::SeqCst);
            self.inner
                .lock()
                .expect("lock")
                .insert(key.to_string(), val);
        }

        fn invalidate_all(&self) {
            self.inner.lock().expect("lock").clear();
        }
    }

    /// A fixed `SystemTime` well before `now()` — avoids sub-second mtime races
    /// and keeps cache keys deterministic across calls within one test.
    fn fixed_time() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    /// A `SystemTime` after [`fixed_time`] — used to simulate file modification.
    fn bumped_time() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_700_000_100)
    }

    /// Pins the file mtime to [`fixed_time`] so the cache key is stable across
    /// calls within a single test.
    fn pin_mtime(path: &Path) {
        let file = fs::File::open(path).expect("open for set_modified");
        file.set_modified(fixed_time())
            .expect("set_modified");
    }

    #[test]
    fn cached_hash_miss_then_hit_returns_same_value() {
        let tmp = NamedTempFile::new().expect("NamedTempFile");
        fs::write(tmp.path(), b"hello world").expect("write");
        pin_mtime(tmp.path());

        let cache = CountingCache::new();

        // First call: cache miss → compute + store.
        let h1 = compute_file_hash_cached(tmp.path(), Some(&cache)).expect("hash");
        assert_eq!(cache.gets(), 1, "first call should query cache (miss)");
        assert_eq!(cache.sets(), 1, "first call should store in cache");

        // Second call: cache hit → return cached value, no new store.
        let h2 = compute_file_hash_cached(tmp.path(), Some(&cache)).expect("hash");
        assert_eq!(h1, h2, "second call should return same hash");
        assert_eq!(cache.gets(), 2, "second call should query cache (hit)");
        assert_eq!(cache.sets(), 1, "second call should NOT store (hit)");

        // Cached value must match uncached computation.
        let uncached = compute_file_hash(tmp.path()).expect("hash");
        assert_eq!(h1, uncached);
    }

    #[test]
    fn cached_hash_with_none_cache_works_like_uncached() {
        let tmp = NamedTempFile::new().expect("NamedTempFile");
        fs::write(tmp.path(), b"hello world").expect("write");

        let cached = compute_file_hash_cached(tmp.path(), None).expect("hash");
        let uncached = compute_file_hash(tmp.path()).expect("hash");
        assert_eq!(cached, uncached);
    }

    #[test]
    fn cached_hash_invalidates_when_mtime_changes() {
        let tmp = NamedTempFile::new().expect("NamedTempFile");
        fs::write(tmp.path(), b"old content").expect("write");
        pin_mtime(tmp.path());

        let cache = CountingCache::new();
        let h1 = compute_file_hash_cached(tmp.path(), Some(&cache)).expect("hash");
        assert_eq!(h1, compute_content_hash(b"old content"));
        assert_eq!(cache.gets(), 1);
        assert_eq!(cache.sets(), 1);

        // Modify content + bump mtime → cache key changes → miss.
        fs::write(tmp.path(), b"new content").expect("write");
        let file = fs::File::open(tmp.path()).expect("open");
        file.set_modified(bumped_time()).expect("set_modified");
        drop(file);

        let h2 = compute_file_hash_cached(tmp.path(), Some(&cache)).expect("hash");
        assert_ne!(h1, h2, "mtime change should invalidate cache");
        assert_eq!(h2, compute_content_hash(b"new content"));
        assert_eq!(cache.gets(), 2, "second call: miss → get");
        assert_eq!(cache.sets(), 2, "second call: store → set");
    }

    #[test]
    fn cached_hash_stores_correct_value() {
        let tmp = NamedTempFile::new().expect("NamedTempFile");
        fs::write(tmp.path(), b"hello world").expect("write");
        pin_mtime(tmp.path());

        let cache = CountingCache::new();
        let h = compute_file_hash_cached(tmp.path(), Some(&cache)).expect("hash");

        let snap = cache.snapshot();
        assert_eq!(snap.len(), 1, "exactly one entry should be cached");
        let (_key, val) = snap.iter().next().expect("one entry");
        assert_eq!(val, h.as_bytes(), "cached value should be hash string bytes");
        assert_eq!(h, compute_content_hash(b"hello world"));
    }
}
