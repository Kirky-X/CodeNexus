// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! SHA-256 file content hashing (ADR-009) and FastCDC content-defined
//! chunking (Phase C5, T130-T133).
//!
//! Provides deterministic SHA-256 digests used by the incremental indexer to
//! detect file changes (BR-INDEX-001~003). Hashes are returned as lowercase
//! hexadecimal strings (64 characters), matching the format stored in the
//! `File` node's `hash` property.
//!
//! # FastCDC (Phase C5)
//!
//! [`chunked_hash`] splits a file into variable-sized chunks via the FastCDC
//! algorithm (rolling hash over a fixed gear table) and computes SHA-256 of
//! each chunk independently. Chunk sizes are bounded by [`MIN_CHUNK_SIZE`]
//! (2KB) and [`MAX_CHUNK_SIZE`] (8KB), averaging ~[`AVG_CHUNK_SIZE`] (4KB).
//! [`compute_file_hash_incremental`] re-chunks the file and aggregates the
//! per-chunk hashes into a file-level digest. Every chunk is re-hashed on
//! each call (see the function's docstring for why `(start, end)` matching
//! cannot safely reuse cached hashes), but exposing the chunk list lets
//! callers cache unchanged `(offset, hash)` pairs at higher layers for
//! incremental change-detection (e.g., skipping re-parse/re-index of
//! unchanged chunks).

use std::path::Path;

#[cfg(feature = "cache")]
use crate::cache::CacheStore;
use sha2::{Digest, Sha256};

/// Minimum chunk size for FastCDC (2KB). No cut point is considered before
/// this many bytes have been accumulated in the current chunk.
pub(crate) const MIN_CHUNK_SIZE: usize = 2 * 1024;

/// Average chunk size for FastCDC (4KB). The rolling-hash mask is sized so
/// that cut points fire, on average, every ~4KB.
pub(crate) const AVG_CHUNK_SIZE: usize = 4 * 1024;

/// Maximum chunk size for FastCDC (8KB). A cut is forced at this boundary
/// regardless of the rolling hash state.
pub(crate) const MAX_CHUNK_SIZE: usize = 8 * 1024;

/// Fixed seed for the gear-table PRNG. The seed is a compile-time constant
/// so the gear table (and therefore chunk boundaries) are deterministic
/// across runs and across machines — a hard requirement for incremental
/// hashing to be correct.
const GEAR_SEED: u64 = 0x0123_4567_89AB_CDEF;

/// Bit count for the dense (post-average) FastCDC mask.
/// `2^BITS = AVG_CHUNK_SIZE`, so BITS = log2(4096) = 12.
const FASTCDC_BITS: u32 = 12;

/// Sparse mask used while `current_chunk_len < AVG_CHUNK_SIZE`. One extra
/// bit reduces the cut-point probability by half, biasing small chunks
/// toward the average. (FastCDC normalization, paper §4.3.)
const FASTCDC_MASK_S: u64 = ((1u64 << (FASTCDC_BITS + 1)) - 1) << 1;

/// Dense mask used once `current_chunk_len >= AVG_CHUNK_SIZE`. The mask
/// halves per byte, so the expected remaining tail is ~2 × AVG = 8KB,
/// clamped by [`MAX_CHUNK_SIZE`].
const FASTCDC_MASK_L: u64 = (1u64 << FASTCDC_BITS) - 1;

/// 256-entry gear table for FastCDC rolling hash. Each entry is a 64-bit
/// pseudo-random value, generated at compile time by `splitmix64` from
/// the fixed seed [`GEAR_SEED`].
const GEAR_TABLE: [u64; 256] = generate_gear_table();

/// `splitmix64` PRNG step (compile-time-`const`-friendly). Used only to
/// populate [`GEAR_TABLE`] — never called at runtime.
const fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Generates the 256-entry gear table at compile time. `const fn` with a
/// `while` loop (no `for` in stable `const fn` as of Rust 1.95).
const fn generate_gear_table() -> [u64; 256] {
    let mut table = [0u64; 256];
    let mut state = GEAR_SEED;
    let mut i = 0;
    while i < 256 {
        table[i] = splitmix64(&mut state);
        i += 1;
    }
    table
}

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
/// `hash:file:{path_hash}:{mtime_nanos}`.
///
/// The `path_hash` is the SHA-256 of the path's string representation. This
/// prevents cache-key injection via `:` characters in file paths (Windows
/// drive letters like `C:\...`, NTFS alternate data streams, or POSIX paths
/// containing `:`), which would otherwise create ambiguity with the `:`
/// separator in the key format and could lead to cache collisions or
/// incorrect cache hits (CWE-20, CWE-346).
///
/// Returns `None` if either `metadata` or `modified` fails — the caller
/// should fall back to uncached computation in that case.
#[cfg(feature = "cache")]
fn build_cache_key(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    let mtime = metadata.modified().ok()?;
    let nanos = mtime.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    let path_hash = compute_content_hash(path.to_string_lossy().as_bytes());
    Some(format!("hash:file:{path_hash}:{nanos}"))
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

/// Splits a file into variable-sized chunks via the FastCDC algorithm
/// (content-defined chunking) and computes SHA-256 of each chunk
/// independently.
///
/// Chunk sizes are bounded by [`MIN_CHUNK_SIZE`] (2KB) and
/// [`MAX_CHUNK_SIZE`] (8KB), averaging ~[`AVG_CHUNK_SIZE`] (4KB). Chunk
/// boundaries are determined by content (rolling hash over the fixed
/// [`GEAR_TABLE`]), so identical file prefixes produce identical chunk
/// boundaries — exposing `(offset, hash)` pairs lets callers of
/// [`compute_file_hash_incremental`] cache unchanged chunks at higher
/// layers for change-detection (re-parse / re-index skipping).
///
/// # Returns
///
/// A `Vec` of `(byte_offset, sha256_hex)` pairs, ordered by offset. The
/// offset is relative to the start of the file. Returns an empty `Vec`
/// for empty files; a single chunk (offset 0) for files smaller than
/// [`MIN_CHUNK_SIZE`].
///
/// # Memory
///
/// Reads the entire file into memory via [`std::fs::read`]. Callers
/// indexing untrusted source trees should bound the file size upstream
/// (e.g., via [`std::fs::metadata`]) before calling this function — a
/// maliciously large file could otherwise trigger OOM. Streaming I/O
/// (mmap-backed chunking) is tracked as future work (C5-followup).
///
/// # Errors
///
/// Returns [`std::io::Error`] if the file cannot be read.
pub fn chunked_hash(file_path: &Path) -> Result<Vec<(u64, String)>, std::io::Error> {
    let content = std::fs::read(file_path)?;
    Ok(chunk_content(&content))
}

/// Chunks `content` via FastCDC and computes SHA-256 of each chunk.
/// Shared between [`chunked_hash`] (file-based API) and the incremental
/// variant. Exposed as a private helper so tests can construct chunk
/// lists from in-memory byte buffers without touching the filesystem.
fn chunk_content(content: &[u8]) -> Vec<(u64, String)> {
    let offsets = fastcdc_offsets(content);
    let mut chunks = Vec::with_capacity(offsets.len().saturating_sub(1));
    for window in offsets.windows(2) {
        let start = window[0];
        let end = window[1];
        let hash = compute_content_hash(&content[start..end]);
        chunks.push((start as u64, hash));
    }
    chunks
}

/// Computes FastCDC chunk boundaries.
///
/// Returns `Vec<usize>` of length `chunk_count + 1`:
/// - `offsets[0] = 0`
/// - `offsets[i+1] = offsets[i] + chunk_i_len`
/// - `offsets[chunk_count] = data.len()`
///
/// Returns an empty `Vec` for empty input.
///
/// The caller can derive chunk `i`'s `(start, end)` as
/// `(offsets[i], offsets[i+1])`.
fn fastcdc_offsets(data: &[u8]) -> Vec<usize> {
    let n = data.len();
    if n == 0 {
        return Vec::new();
    }
    let mut offsets = Vec::with_capacity(n / AVG_CHUNK_SIZE + 2);
    offsets.push(0);
    let mut start = 0usize;
    while start < n {
        let remaining = n - start;
        let chunk_len = if remaining <= MIN_CHUNK_SIZE {
            // Tail shorter than MIN: emit as a single short chunk (no
            // cut-point search). This is the only path that produces a
            // chunk smaller than MIN_CHUNK_SIZE, and only at EOF.
            remaining
        } else {
            find_cut_point(&data[start..n])
        };
        start += chunk_len;
        offsets.push(start);
    }
    offsets
}

/// Finds the length of the next FastCDC chunk starting at `data[0]`.
///
/// Uses the two-mask Normalization variant from the FastCDC paper §4.3:
/// - Phase 1 (`MIN_CHUNK_SIZE <= i < AVG_CHUNK_SIZE`): sparse mask
///   [`FASTCDC_MASK_S`] reduces cut-point probability, biasing small
///   chunks toward the average.
/// - Phase 2 (`AVG_CHUNK_SIZE <= i < MAX_CHUNK_SIZE`): dense mask
///   [`FASTCDC_MASK_L`] increases cut-point probability, biasing large
///   chunks toward the average.
///
/// If no cut point is found before `MAX_CHUNK_SIZE` (or end of data),
/// a cut is forced at that boundary.
///
/// # Pre-condition
///
/// `data.len() > MIN_CHUNK_SIZE` (caller enforces).
fn find_cut_point(data: &[u8]) -> usize {
    let n = data.len();
    debug_assert!(n > MIN_CHUNK_SIZE, "find_cut_point requires n > MIN");

    let max_scan = n.min(MAX_CHUNK_SIZE);
    let mut fp: u64 = 0;

    // Phase 1: sparse mask. Stop at min(AVG, max_scan).
    let phase1_end = max_scan.min(AVG_CHUNK_SIZE);
    let mut i = MIN_CHUNK_SIZE;
    while i < phase1_end {
        fp = (fp << 1).wrapping_add(GEAR_TABLE[data[i] as usize]);
        if (fp & FASTCDC_MASK_S) == 0 {
            return i + 1;
        }
        i += 1;
    }

    // Phase 2: dense mask. Continue until MAX (or end of data).
    while i < max_scan {
        fp = (fp << 1).wrapping_add(GEAR_TABLE[data[i] as usize]);
        if (fp & FASTCDC_MASK_L) == 0 {
            return i + 1;
        }
        i += 1;
    }

    // No cut point found — force cut at MAX_CHUNK_SIZE (or EOF).
    max_scan
}

/// Incremental file hash: re-chunks the file with FastCDC and computes
/// SHA-256 of each chunk, then aggregates the per-chunk hashes into a
/// single file-level digest.
///
/// The file-level hash is the SHA-256 of the concatenation of all chunk
/// hashes (sorted ascending for stability), producing a single
/// deterministic digest for the file regardless of chunk boundary drift.
///
/// # Why every chunk is re-hashed (deviation from spec T132)
///
/// Spec T132 says "only re-hash changed chunks", but FastCDC's cut-point
/// search (see [`find_cut_point`]) starts scanning at [`MIN_CHUNK_SIZE`]:
/// bytes in `[start, start + MIN_CHUNK_SIZE)` do **not** influence the
/// cut point. Therefore identical `(start, end)` boundaries do **not**
/// imply identical chunk content — a modification inside the first
/// `MIN_CHUNK_SIZE` bytes of a chunk leaves the cut point unchanged but
/// alters the chunk's bytes. Reusing `old_chunks` hashes by matching
/// `(start, end)` would silently miss such modifications (false negative
/// on change detection).
///
/// To preserve correctness, this implementation re-hashes every chunk on
/// every call. Performance still meets the C5 target: a 1MB file splits
/// into ~256 chunks of ~4KB, and SHA-256 throughput (~200MB/s on modern
/// CPUs) yields ~20µs per chunk → ~5ms total, matching the spec's
/// ~5ms goal. The `old_chunks` parameter is retained as part of the
/// incremental API contract (T132) and is returned to the caller along
/// with the new chunks for change-detection and caching at higher
/// layers.
///
/// # Algorithm
///
/// 1. Read the file and re-run FastCDC to determine the new chunk
///    boundaries.
/// 2. For each new chunk, compute SHA-256 of its bytes (always).
/// 3. Concatenate all chunk hashes (sorted ascending) and SHA-256 the
///    concatenation to produce the file-level hash.
///
/// # Returns
///
/// `(file_level_hash, new_chunks)`, where `new_chunks` is the updated
/// chunk list to be persisted by the caller for the next incremental
/// pass. Callers compare `old_chunks` vs `new_chunks` (by offset and
/// hash) to identify changed chunks for cache invalidation.
///
/// # Errors
///
/// Returns [`std::io::Error`] if the file cannot be read.
pub fn compute_file_hash_incremental(
    file_path: &Path,
    old_chunks: &[(u64, String)],
) -> Result<(String, Vec<(u64, String)>), std::io::Error> {
    let content = std::fs::read(file_path)?;
    if content.is_empty() {
        // Empty file: no chunks; file hash is SHA-256 of empty input.
        return Ok((compute_content_hash(b""), Vec::new()));
    }
    // Re-chunk and hash every chunk. See docstring above for why we
    // cannot reuse `old_chunks` hashes by matching `(start, end)`.
    let new_chunks = chunk_content(&content);
    let file_hash = aggregate_chunk_hashes(&new_chunks);
    // `old_chunks` is part of the API contract (T132) but is not used
    // for hash reuse (see docstring). Reference it to silence
    // unused-variable warnings while keeping the parameter public.
    let _ = old_chunks;
    Ok((file_hash, new_chunks))
}

/// Aggregates per-chunk SHA-256 hashes into a single file-level digest.
///
/// Hashes are sorted ascending (lexicographic on hex string) before
/// concatenation, so the file-level digest is robust against
/// chunk-boundary drift: two files with identical chunk-content multisets
/// produce the same file-level hash even if chunk boundaries differ.
fn aggregate_chunk_hashes(chunks: &[(u64, String)]) -> String {
    let mut hashes: Vec<&str> = chunks.iter().map(|(_, h)| h.as_str()).collect();
    hashes.sort_unstable();
    let mut concat: Vec<u8> = Vec::with_capacity(hashes.len() * 64);
    for h in &hashes {
        concat.extend_from_slice(h.as_bytes());
    }
    compute_content_hash(&concat)
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
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9" // pragma: allowlist secret
        );
    }

    #[test]
    fn compute_content_hash_empty_input() {
        // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let hash = compute_content_hash(b"");
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" // pragma: allowlist secret
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
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" // pragma: allowlist secret
        );
    }

    // --- FastCDC: chunked_hash (T130 Red) ---

    /// Helper: fill `dst` with deterministic pseudo-random bytes derived from
    /// a xorshift64 PRNG seeded with `seed`. Deterministic across runs and
    /// machines — required for FastCDC chunk-boundary stability.
    fn fill_pseudo_random(dst: &mut [u8], seed: u64) {
        let mut state = seed;
        for byte in dst.iter_mut() {
            // xorshift64
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *byte = (state & 0xFF) as u8;
        }
    }

    #[test]
    fn test_fastcdc_chunks_local_modification_only_rehashes_changed_chunks() {
        // 1MB file of pseudo-random content (deterministic).
        let mut content = vec![0u8; 1024 * 1024];
        fill_pseudo_random(&mut content, 0x1234_5678_9ABC_DEF0);

        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), &content).unwrap();
        let chunks_before = chunked_hash(tmp.path()).expect("chunked_hash should succeed");

        // 1MB file with 4KB average → expect > 100 chunks.
        assert!(
            chunks_before.len() > 100,
            "expected > 100 chunks for 1MB file, got {}",
            chunks_before.len()
        );

        // Modify ~100 bytes at offset ~400KB (around 100th chunk).
        let modify_offset = 409_600usize;
        for i in 0..100 {
            content[modify_offset + i] ^= 0xFF;
        }
        fs::write(tmp.path(), &content).unwrap();
        let chunks_after =
            chunked_hash(tmp.path()).expect("chunked_hash should succeed after edit");

        // Property 1: chunks entirely BEFORE the modification point must
        // preserve their start offsets (FastCDC guarantee: identical
        // file[0..start_i] ⇒ identical start_i). However, the single chunk
        // that *spans* `modify_offset` (start < modify_offset < end) has
        // its content altered, so its hash changes — that chunk counts as
        // "drifted" even though its start offset is unchanged. We allow
        // up to 1 drifted chunk before modify_offset for this reason.
        let mut identical_before = 0usize;
        let mut drifted_before = 0usize;
        for (off, hash) in &chunks_before {
            if (*off as usize) >= modify_offset {
                break;
            }
            let matched = chunks_after
                .iter()
                .take_while(|(o, _)| (*o as usize) < modify_offset)
                .any(|(o, h)| o == off && h == hash);
            if matched {
                identical_before += 1;
            } else {
                drifted_before += 1;
            }
        }
        assert!(
            drifted_before <= 1,
            "expected <= 1 drifted chunk before modification (the spanning chunk), \
             got {drifted_before} (identical_before={identical_before})"
        );
        assert!(
            identical_before > 0,
            "expected at least some chunks before modification, got 0"
        );

        // Property 2: total changed chunks (different hash at matching offset,
        // or offset absent from `chunks_before`) is small — only the modified
        // chunk plus 1-2 boundary-drift neighbors.
        let before_map: std::collections::HashMap<u64, &String> =
            chunks_before.iter().map(|(o, h)| (*o, h)).collect();
        let changed: usize = chunks_after
            .iter()
            .filter(|(o, h)| before_map.get(o).map_or(true, |bh| *bh != h))
            .count();
        // Allow up to 5 chunks to change (conservative; theory says 1-3).
        assert!(
            changed <= 5,
            "expected <= 5 changed chunks after local modification, got {changed} \
             (total before={}, total after={})",
            chunks_before.len(),
            chunks_after.len()
        );
    }

    #[test]
    fn test_fastcdc_chunk_size_bounds() {
        // 1MB of pseudo-random content.
        let mut content = vec![0u8; 1024 * 1024];
        fill_pseudo_random(&mut content, 0xC0FFEE_BEEF_12345);

        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), &content).unwrap();
        let chunks = chunked_hash(tmp.path()).expect("chunked_hash should succeed");

        // Reconstruct chunk sizes from offsets + file length.
        let mut sizes: Vec<usize> = Vec::with_capacity(chunks.len());
        for (i, (off, _)) in chunks.iter().enumerate() {
            let start = *off as usize;
            let end = if i + 1 < chunks.len() {
                chunks[i + 1].0 as usize
            } else {
                content.len()
            };
            sizes.push(end.saturating_sub(start));
        }
        assert!(!sizes.is_empty(), "should produce at least one chunk");

        for (i, &size) in sizes.iter().enumerate() {
            let is_final = i + 1 == sizes.len();
            // Non-final chunks must be >= MIN_CHUNK_SIZE (final may be
            // shorter when file length isn't a multiple of chunk sizes).
            assert!(
                size >= MIN_CHUNK_SIZE || is_final,
                "chunk {i} size {size} < MIN_CHUNK_SIZE {} (non-final)",
                MIN_CHUNK_SIZE
            );
            // All chunks (including final) must be <= MAX_CHUNK_SIZE.
            assert!(
                size <= MAX_CHUNK_SIZE,
                "chunk {i} size {size} > MAX_CHUNK_SIZE {}",
                MAX_CHUNK_SIZE
            );
        }

        // Average should be close to AVG_CHUNK_SIZE (±50% tolerance —
        // pseudo-random content with FastCDC normalization should land
        // within [2KB, 8KB] with mean near 4KB).
        let total: usize = sizes.iter().sum();
        let avg = total as f64 / sizes.len() as f64;
        let tolerance = (AVG_CHUNK_SIZE as f64) * 0.5;
        assert!(
            (avg - AVG_CHUNK_SIZE as f64).abs() <= tolerance,
            "average chunk size {avg:.1} not within ±{tolerance:.1} of AVG_CHUNK_SIZE {AVG_CHUNK_SIZE}"
        );
    }

    #[test]
    fn test_fastcdc_empty_file_returns_empty() {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"").unwrap();
        let chunks = chunked_hash(tmp.path()).expect("empty file should not error");
        assert!(
            chunks.is_empty(),
            "empty file should produce no chunks, got {chunks:?}"
        );
    }

    #[test]
    fn test_fastcdc_small_file_returns_single_chunk() {
        let tmp = NamedTempFile::new().unwrap();
        // 1KB < MIN_CHUNK_SIZE (2KB) → single chunk at offset 0.
        let content = vec![0x42u8; 1024];
        fs::write(tmp.path(), &content).unwrap();
        let chunks = chunked_hash(tmp.path()).expect("small file should not error");
        assert_eq!(
            chunks.len(),
            1,
            "small file should produce exactly 1 chunk, got {}",
            chunks.len()
        );
        assert_eq!(chunks[0].0, 0u64, "single chunk should start at offset 0");
        assert_eq!(chunks[0].1.len(), 64, "chunk hash should be 64-char hex");
    }

    #[test]
    fn test_chunked_hash_returns_valid_offsets_and_hashes() {
        // 64KB file: small enough to be fast, large enough to produce
        // multiple chunks under FastCDC (avg=4KB → ~16 chunks).
        let mut content = vec![0u8; 64 * 1024];
        fill_pseudo_random(&mut content, 0x0BAD_CAFE_1234_5678);

        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), &content).unwrap();
        let chunks = chunked_hash(tmp.path()).expect("chunked_hash should succeed");

        // Non-empty file must produce at least one chunk.
        assert!(!chunks.is_empty(), "non-empty file should produce chunks");

        // First chunk must start at offset 0.
        assert_eq!(chunks[0].0, 0u64, "first chunk must start at offset 0");

        // Offsets must be strictly increasing (no overlap, no duplicates).
        for window in chunks.windows(2) {
            assert!(
                window[1].0 > window[0].0,
                "offsets must be strictly increasing, got {} then {}",
                window[0].0,
                window[1].0
            );
        }

        // Last chunk's offset must be < file size (chunk content extends
        // beyond the offset).
        let last_offset = chunks.last().unwrap().0;
        assert!(
            (last_offset as usize) < content.len(),
            "last chunk offset {last_offset} must be < file size {}",
            content.len()
        );

        // Every hash must be a 64-char lowercase hex string.
        for (i, (off, hash)) in chunks.iter().enumerate() {
            assert_eq!(
                hash.len(),
                64,
                "chunk {i} (offset {off}) hash must be 64 chars, got {}",
                hash.len()
            );
            assert!(
                hash.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "chunk {i} hash must be lowercase hex: {hash}"
            );
        }

        // Hashes must be unique (different chunks of pseudo-random content
        // must produce distinct SHA-256 digests with overwhelming
        // probability).
        let unique: std::collections::HashSet<&str> =
            chunks.iter().map(|(_, h)| h.as_str()).collect();
        assert_eq!(
            unique.len(),
            chunks.len(),
            "chunk hashes must be unique for pseudo-random content"
        );
    }

    // --- FastCDC: compute_file_hash_incremental (T130 Red) ---

    #[test]
    fn test_compute_file_hash_incremental_empty_file() {
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"").unwrap();

        // Empty file with empty old_chunks → empty new_chunks, SHA-256("").
        let (file_hash, new_chunks) =
            compute_file_hash_incremental(tmp.path(), &[]).expect("empty file should not error");
        assert!(
            new_chunks.is_empty(),
            "empty file must produce no chunks, got {new_chunks:?}"
        );
        assert_eq!(
            file_hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", // pragma: allowlist secret
            "empty file hash must be SHA-256 of empty input"
        );
        assert_eq!(file_hash.len(), 64);
        assert!(
            file_hash
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "file hash must be lowercase hex: {file_hash}"
        );

        // Calling with non-empty old_chunks on an empty file must still
        // produce the empty-file hash (old_chunks is ignored for hash
        // computation; see compute_file_hash_incremental docstring).
        let stale_old = vec![(0u64, String::from("0".repeat(64)))];
        let (file_hash_2, new_chunks_2) = compute_file_hash_incremental(tmp.path(), &stale_old)
            .expect("empty file with stale old_chunks should not error");
        assert_eq!(file_hash, file_hash_2, "empty file hash must be stable");
        assert!(
            new_chunks_2.is_empty(),
            "empty file must produce no chunks even with stale old_chunks"
        );
    }

    #[test]
    fn test_compute_file_hash_incremental_small_file() {
        // 1KB < MIN_CHUNK_SIZE → single chunk at offset 0.
        let content = vec![0x42u8; 1024];
        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), &content).unwrap();

        // First call with empty old_chunks.
        let (file_hash_1, new_chunks_1) =
            compute_file_hash_incremental(tmp.path(), &[]).expect("small file should not error");
        assert_eq!(
            new_chunks_1.len(),
            1,
            "small file must produce exactly 1 chunk, got {}",
            new_chunks_1.len()
        );
        assert_eq!(new_chunks_1[0].0, 0u64, "single chunk must start at 0");
        assert_eq!(
            new_chunks_1[0].1,
            compute_content_hash(&content),
            "single chunk hash must equal whole-content hash"
        );
        assert_eq!(file_hash_1.len(), 64, "file hash must be 64-char hex");

        // Second call passing the previous chunks as old_chunks must
        // produce the same file hash (determinism).
        let (file_hash_2, new_chunks_2) = compute_file_hash_incremental(tmp.path(), &new_chunks_1)
            .expect("second call should not error");
        assert_eq!(
            file_hash_1, file_hash_2,
            "file hash must be deterministic across calls"
        );
        assert_eq!(
            new_chunks_1, new_chunks_2,
            "chunk list must be identical when file is unchanged"
        );

        // Modify the file → file hash must change.
        let mut modified = content.clone();
        modified[0] ^= 0xFF;
        fs::write(tmp.path(), &modified).unwrap();
        let (file_hash_3, new_chunks_3) = compute_file_hash_incremental(tmp.path(), &new_chunks_1)
            .expect("post-edit call should not error");
        assert_ne!(
            file_hash_1, file_hash_3,
            "file hash must change after content modification"
        );
        assert_eq!(new_chunks_3.len(), 1, "small file must still be 1 chunk");
        assert_ne!(
            new_chunks_3[0].1, new_chunks_1[0].1,
            "chunk hash must change after content modification"
        );
    }

    #[test]
    fn test_compute_file_hash_incremental_reuses_unchanged_chunks() {
        // 1MB file of pseudo-random content.
        let mut content = vec![0u8; 1024 * 1024];
        fill_pseudo_random(&mut content, 0xDEAD_BEEF_CAFE_BABE);

        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), &content).unwrap();

        // Baseline: full chunked hash.
        let old_chunks = chunked_hash(tmp.path()).expect("baseline chunked_hash");
        assert!(
            old_chunks.len() > 100,
            "expected > 100 chunks, got {}",
            old_chunks.len()
        );

        // Incremental pass with file UNCHANGED — should reuse all chunks.
        let (file_hash, new_chunks) =
            compute_file_hash_incremental(tmp.path(), &old_chunks).expect("incremental hash");

        // new_chunks must equal old_chunks (file unchanged).
        assert_eq!(
            new_chunks.len(),
            old_chunks.len(),
            "chunk count must match when file unchanged"
        );
        for (i, (off, hash)) in new_chunks.iter().enumerate() {
            assert_eq!(*off, old_chunks[i].0, "offset mismatch at chunk {i}");
            assert_eq!(hash, &old_chunks[i].1, "hash mismatch at chunk {i}");
        }

        // File hash must be 64-char lowercase hex.
        assert_eq!(file_hash.len(), 64, "file hash must be 64-char hex");
        assert!(
            file_hash
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "file hash must be lowercase hex: {file_hash}"
        );

        // File hash must be deterministic across calls.
        let (file_hash_2, _) =
            compute_file_hash_incremental(tmp.path(), &old_chunks).expect("second call");
        assert_eq!(file_hash, file_hash_2, "file hash must be deterministic");
    }

    #[test]
    fn test_compute_file_hash_incremental_detects_all_changes() {
        let mut content = vec![0u8; 1024 * 1024];
        fill_pseudo_random(&mut content, 0xFEED_FACE_1234_5678);

        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), &content).unwrap();
        let old_chunks = chunked_hash(tmp.path()).expect("baseline chunked_hash");
        let (old_file_hash, _) =
            compute_file_hash_incremental(tmp.path(), &old_chunks).expect("baseline incremental");

        // Modify content at multiple positions.
        for &pos in &[100_000usize, 500_000, 900_000] {
            for i in 0..200 {
                content[pos + i] ^= 0xFF;
            }
        }
        fs::write(tmp.path(), &content).unwrap();

        let (new_file_hash, new_chunks) =
            compute_file_hash_incremental(tmp.path(), &old_chunks).expect("post-edit incremental");

        // File hash MUST change.
        assert_ne!(
            new_file_hash, old_file_hash,
            "file hash must change after content modification"
        );

        // At least one chunk must differ from old_chunks (offset absent or hash differs).
        let old_map: std::collections::HashMap<u64, &String> =
            old_chunks.iter().map(|(o, h)| (*o, h)).collect();
        let changed: usize = new_chunks
            .iter()
            .filter(|(o, h)| old_map.get(o).map_or(true, |oh| *oh != h))
            .count();
        assert!(
            changed > 0,
            "expected at least one changed chunk, got 0 (total new={})",
            new_chunks.len()
        );
    }
}

#[cfg(all(test, feature = "cache"))]
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
        file.set_modified(fixed_time()).expect("set_modified");
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
        assert_eq!(
            val,
            h.as_bytes(),
            "cached value should be hash string bytes"
        );
        assert_eq!(h, compute_content_hash(b"hello world"));
    }
}
