// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Incremental indexing via BLAKE3 hash diffing (ADR-009).
//!
//! Compares the set of source files on disk against the `(path, hash)` pairs
//! stored in the database for a project, classifying each file as
//! [`FileDiff::changed`], [`FileDiff::added`], [`FileDiff::unchanged`], or
//! [`FileDiff::deleted`]. The pipeline uses this classification to skip
//! unchanged files, delete nodes for removed files,
//! and force a full re-parse when `--force` is set.

use std::collections::HashMap;

use rayon::prelude::*;
use tracing::warn;

use crate::discover::FileInfo;
use crate::index::hash::compute_file_hash;

/// The diff between on-disk files and the database's stored hashes.
///
/// Produced by [`diff_files`]. Each bucket drives a distinct pipeline stage:
///
/// - `changed` → re-parse and replace nodes for these files.
/// - `added` → parse and insert nodes for these files.
/// - `unchanged` → skip.
/// - `deleted` → remove nodes and edges for these paths.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileDiff {
    /// Files whose hash differs from the stored hash (re-parse required).
    pub changed: Vec<FileInfo>,
    /// Files present on disk but absent from the database (new files).
    pub added: Vec<FileInfo>,
    /// Files whose hash matches the stored hash (skip).
    pub unchanged: Vec<FileInfo>,
    /// Paths present in the database but absent on disk.
    pub deleted: Vec<String>,
    /// BLAKE3 hashes computed during classification, keyed by relative path.
    ///
    /// Downstream consumers (e.g. `build_file_nodes`) reuse these instead of
    /// re-reading every file from disk — previously each changed file was
    /// read 2× for hashing (diff + File-node build), plus a separate read
    /// for line counting and one for parsing. Files that were skipped during
    /// classification (symlink / oversized) are absent from this map.
    pub hashes: HashMap<String, String>,
}

impl FileDiff {
    /// Creates an empty `FileDiff` (all buckets empty).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if every bucket is empty (no work to do).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty()
            && self.added.is_empty()
            && self.unchanged.is_empty()
            && self.deleted.is_empty()
    }

    /// Returns the total number of files across all buckets.
    ///
    /// `deleted` counts as one per path; the other buckets count one per
    /// [`FileInfo`].
    #[must_use]
    pub fn total(&self) -> usize {
        self.changed.len() + self.added.len() + self.unchanged.len() + self.deleted.len()
    }

    /// Returns the files that need to be parsed (changed + added).
    #[must_use]
    pub fn to_parse(&self) -> Vec<&FileInfo> {
        self.changed.iter().chain(self.added.iter()).collect()
    }
}

/// Per-file classification produced by parallel hash diffing.
///
/// Used internally by [`diff_files`] to classify each disk file before
/// bucketing into [`FileDiff`]. Computed in parallel by rayon, then
/// collected in order to preserve disk traversal order.
enum FileClass {
    Changed,
    Added,
    Unchanged,
}

/// Computes the [`FileDiff`] between `disk_files` and `db_hashes`.
///
/// # Arguments
///
/// * `disk_files` - Files discovered on disk by [`crate::discover::Walker`].
/// * `db_hashes` - `(path, hash)` pairs loaded from the database via
///   [`crate::storage::Repository::get_all_file_hashes`].
/// * `force` - When `true`, every disk file is classified as `changed`
///   regardless of its hash (`--force`). `deleted` is still
///   computed.
///
/// # Errors
///
/// Returns [`std::io::Error`] if a file's hash cannot be computed (e.g. the
/// file was deleted between discovery and hashing).
///
/// # Classification rules
///
/// - hash matches DB → `unchanged` (skip).
/// - in DB but not on disk → `deleted`.
/// - `force=true` → all disk files go to `changed`.
/// - New file (not in DB) → `added`.
/// - Hash differs → `changed`.
///
/// # Parallelism
///
/// Hash computation and per-file classification run in parallel via rayon
/// (`par_iter`). BLAKE3 hashing is I/O-bound (file read) + CPU-bound
/// (digest), so rayon's thread pool overlaps disk I/O across files. Results
/// are collected in `disk_files` order to preserve traversal order.
///
/// [`FileClass`]: self::FileClass
pub fn diff_files(
    disk_files: &[FileInfo],
    db_hashes: &[(String, String)],
    force: bool,
) -> Result<FileDiff, std::io::Error> {
    Ok(diff_files_with_hints(disk_files, db_hashes, force, &HashMap::new())?.0)
}

/// A per-file hint enabling the daemon fast path: when `(size, mtime)` match
/// the previous run, the previous BLAKE3 hash is reused without reading the
/// file. One `stat` per file replaces a full read + hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHashHint {
    /// File size in bytes (from `FileInfo`, no extra syscall).
    pub size: u64,
    /// File modification time in nanoseconds since the Unix epoch.
    pub mtime_nanos: i128,
    /// BLAKE3 hash of the content at the observed `(size, mtime)`.
    pub hash: String,
}

/// Returns the file's mtime in nanoseconds since the Unix epoch, or `None`
/// when metadata is unreadable (pre-1970 timestamps clamp to 0).
fn mtime_nanos(path: &std::path::Path) -> Option<i128> {
    let meta = std::fs::metadata(path).ok()?;
    Some(
        meta.modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i128)
            .unwrap_or(0),
    )
}

/// [`diff_files`] with daemon fast-path hints.
///
/// For each disk file: if `hints` contains an entry whose `size` matches
/// `FileInfo.size` and whose `mtime_nanos` matches a single `stat` of the
/// file, the previous hash is reused (no content read). Otherwise the file is
/// read and hashed as before. Alongside the [`FileDiff`], returns the fresh
/// hint map for the *next* run — the caller (daemon `IndexFacade`) stores it
/// process-wide so consecutive incremental batches only pay one `stat` per
/// unchanged file instead of a full read + BLAKE3 pass.
///
/// Correctness contract: a reused hash equals the DB hash ⇒ `Unchanged`
/// (skip); a hint mismatch (touch/rename-restore) falls back to a full hash,
/// never to a wrong classification. The standard mtime+size trade-off
/// applies: content changes that preserve both size and mtime are missed —
/// the same trade-off every incremental build system makes, and `--force`
/// bypasses hints entirely.
/// Per-file outcome of the parallel classification pass: the class, the
/// (reused or freshly computed) hash, and the fresh hint for the next run
/// (`None` when the file was skipped or its mtime was unreadable).
type HashClassification = (FileClass, String, Option<FileHashHint>);

pub fn diff_files_with_hints(
    disk_files: &[FileInfo],
    db_hashes: &[(String, String)],
    force: bool,
    hints: &HashMap<String, FileHashHint>,
) -> Result<(FileDiff, HashMap<String, FileHashHint>), std::io::Error> {
    // Index DB hashes by path for O(1) lookup.
    let mut db_map: HashMap<&str, &str> = HashMap::with_capacity(db_hashes.len());
    for (path, hash) in db_hashes {
        db_map.insert(path.as_str(), hash.as_str());
    }

    // Parallel hash computation + classification (rayon par_iter).
    //
    // BLAKE3 hashing is I/O-bound (file read) + CPU-bound (digest), so
    // rayon's thread pool overlaps disk I/O across files. Each file is
    // classified independently; results are collected in disk_files order
    // (rayon preserves source order on `collect`).
    //
    // `compute_file_hash` rejects
    // symlinks (path traversal) and oversized files (OOM) with
    // `InvalidInput`. We skip those files (warn + treat as unchanged so
    // they don't trigger re-parse) rather than failing the entire scan
    // phase — a single malicious symlink should not block indexing the
    // rest of the project. Other errors (NotFound, PermissionDenied) are
    // propagated as before.
    let classifications: Result<Vec<Option<HashClassification>>, std::io::Error> = disk_files
        .par_iter()
        .map(|file| {
            // Daemon fast path: one `stat` per file; a (size, mtime) hit
            // reuses the previous hash instead of reading the content.
            let mtime = mtime_nanos(&file.path);
            let hint = hints.get(file.relative_path.as_str());
            let cached_hash = match (hint, &mtime) {
                (Some(h), Some(mt)) if !force && h.size == file.size && h.mtime_nanos == *mt => {
                    Some(h.hash.clone())
                }
                _ => None,
            };
            let disk_hash = match cached_hash {
                Some(h) => h,
                None => match compute_file_hash(&file.path) {
                    Ok(h) => h,
                    Err(err) if err.kind() == std::io::ErrorKind::InvalidInput => {
                        // Symlink or oversized file: skip with a warning.
                        // Returning None signals the caller to skip this file.
                        warn!(
                            file = %file.relative_path,
                            error = %err,
                            "skipping file during hash classification \
                             (symlink or exceeds MAX_FILE_SIZE)"
                        );
                        return Ok(None);
                    }
                    Err(err) => return Err(err),
                },
            };
            // Fresh hint for the next run (stat result already in hand).
            let fresh_hint = mtime.map(|mtime_nanos| FileHashHint {
                size: file.size,
                mtime_nanos,
                hash: disk_hash.clone(),
            });
            if force {
                // --force ignores hashes; every disk file is changed.
                // The hash is still carried downstream (File-node build
                // reuses it instead of re-reading the file).
                return Ok(Some((FileClass::Changed, disk_hash, fresh_hint)));
            }
            let class = match db_map.get(file.relative_path.as_str()) {
                None => FileClass::Added,
                Some(db_hash) => {
                    if *db_hash == disk_hash {
                        // Hash matches → skip.
                        FileClass::Unchanged
                    } else {
                        // Hash differs → re-parse.
                        FileClass::Changed
                    }
                }
            };
            Ok(Some((class, disk_hash, fresh_hint)))
        })
        .collect();

    let classifications = classifications?;

    let mut diff = FileDiff::new();
    let mut fresh_hints: HashMap<String, FileHashHint> = HashMap::with_capacity(disk_files.len());
    let mut seen_on_disk: HashMap<&str, ()> = HashMap::with_capacity(disk_files.len());

    // Bucket files in disk traversal order (preserves pre-parallel behavior).
    // Files whose hash computation was skipped (symlink / oversized) are
    // recorded in `seen_on_disk` so they are not mistakenly classified as
    // deleted from the DB, but they are not added to any diff bucket — the
    // pipeline's `build_file_nodes` will warn again when it fails to hash
    // them and skip File node creation.
    for (file, class) in disk_files.iter().zip(classifications) {
        seen_on_disk.insert(file.relative_path.as_str(), ());
        match class {
            Some((FileClass::Changed, hash, hint)) => {
                diff.hashes.insert(file.relative_path.clone(), hash);
                if let Some(hint) = hint {
                    fresh_hints.insert(file.relative_path.clone(), hint);
                }
                diff.changed.push(file.clone());
            }
            Some((FileClass::Added, hash, hint)) => {
                diff.hashes.insert(file.relative_path.clone(), hash);
                if let Some(hint) = hint {
                    fresh_hints.insert(file.relative_path.clone(), hint);
                }
                diff.added.push(file.clone());
            }
            Some((FileClass::Unchanged, hash, hint)) => {
                diff.hashes.insert(file.relative_path.clone(), hash);
                if let Some(hint) = hint {
                    fresh_hints.insert(file.relative_path.clone(), hint);
                }
                diff.unchanged.push(file.clone());
            }
            None => {
                // Skipped during classification — already warned above.
                // Fall through without bucketing.
            }
        }
    }

    // In DB but not on disk → deleted.
    for (path, _) in db_hashes {
        if !seen_on_disk.contains_key(path.as_str()) {
            diff.deleted.push(path.clone());
        }
    }

    Ok((diff, fresh_hints))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Language;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    /// Writes a file at `dir/rel` (creating parent directories as needed) and
    /// returns a `FileInfo` describing it.
    fn make_file(dir: &Path, rel: &str, content: &str, language: Language) -> FileInfo {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap();
        let metadata = fs::metadata(&path).unwrap();
        FileInfo {
            path,
            relative_path: rel.to_string(),
            language: Some(language),
            size: metadata.len(),
        }
    }

    /// Computes the BLAKE3 hash of `dir/rel` for use in DB hash fixtures.
    fn hash_of(dir: &Path, rel: &str) -> String {
        compute_file_hash(&dir.join(rel)).unwrap()
    }

    // --- FileDiff ---

    #[test]
    fn file_diff_new_is_empty() {
        let diff = FileDiff::new();
        assert!(diff.changed.is_empty());
        assert!(diff.added.is_empty());
        assert!(diff.unchanged.is_empty());
        assert!(diff.deleted.is_empty());
        assert!(diff.is_empty());
        assert_eq!(diff.total(), 0);
    }

    #[test]
    fn file_diff_is_empty_false_when_populated() {
        let mut diff = FileDiff::new();
        diff.added.push(FileInfo {
            path: PathBuf::from("/x.rs"),
            relative_path: "x.rs".to_string(),
            language: Some(Language::Rust),
            size: 0,
        });
        assert!(!diff.is_empty());
        assert_eq!(diff.total(), 1);
    }

    #[test]
    fn file_diff_total_sums_all_buckets() {
        use std::path::PathBuf;
        let mut diff = FileDiff::new();
        diff.changed.push(FileInfo {
            path: PathBuf::from("/a.rs"),
            relative_path: "a.rs".to_string(),
            language: Some(Language::Rust),
            size: 0,
        });
        diff.added.push(FileInfo {
            path: PathBuf::from("/b.rs"),
            relative_path: "b.rs".to_string(),
            language: Some(Language::Rust),
            size: 0,
        });
        diff.unchanged.push(FileInfo {
            path: PathBuf::from("/c.rs"),
            relative_path: "c.rs".to_string(),
            language: Some(Language::Rust),
            size: 0,
        });
        diff.deleted.push("d.rs".to_string());
        assert_eq!(diff.total(), 4);
    }

    #[test]
    fn file_diff_to_parse_combines_changed_and_added() {
        use std::path::PathBuf;
        let mut diff = FileDiff::new();
        diff.changed.push(FileInfo {
            path: PathBuf::from("/a.rs"),
            relative_path: "a.rs".to_string(),
            language: Some(Language::Rust),
            size: 0,
        });
        diff.added.push(FileInfo {
            path: PathBuf::from("/b.rs"),
            relative_path: "b.rs".to_string(),
            language: Some(Language::Rust),
            size: 0,
        });
        diff.unchanged.push(FileInfo {
            path: PathBuf::from("/c.rs"),
            relative_path: "c.rs".to_string(),
            language: Some(Language::Rust),
            size: 0,
        });
        let to_parse = diff.to_parse();
        assert_eq!(to_parse.len(), 2);
        let paths: Vec<&str> = to_parse.iter().map(|f| f.relative_path.as_str()).collect();
        assert!(paths.contains(&"a.rs"));
        assert!(paths.contains(&"b.rs"));
    }

    // --- diff_files: all new files → added ---

    #[test]
    fn diff_files_all_new_files_go_to_added() {
        let tmp = TempDir::new().unwrap();
        let f1 = make_file(tmp.path(), "a.rs", "fn a() {}", Language::Rust);
        let f2 = make_file(tmp.path(), "b.rs", "fn b() {}", Language::Rust);
        let disk = vec![f1, f2];
        let db: Vec<(String, String)> = vec![];

        let diff = diff_files(&disk, &db, false).unwrap();

        assert_eq!(diff.added.len(), 2, "both files should be added");
        assert!(diff.changed.is_empty());
        assert!(diff.unchanged.is_empty());
        assert!(diff.deleted.is_empty());
    }

    // --- diff_files_with_hints: daemon fast path ---

    /// Hint hit (size+mtime unchanged) reuses the previous hash without
    /// re-reading, and the fresh hint map round-trips to the next run.
    #[test]
    fn diff_files_with_hints_reuses_unchanged_files() {
        let tmp = TempDir::new().unwrap();
        let f = make_file(tmp.path(), "a.rs", "fn a() {}", Language::Rust);
        let disk = vec![f];
        let db: Vec<(String, String)> = vec![];

        // First run: no hints → full hash; capture fresh hints.
        let (_, fresh) = diff_files_with_hints(&disk, &db, false, &HashMap::new()).unwrap();
        let hash1 = fresh
            .get("a.rs")
            .expect("first run must record a hint")
            .clone();
        assert_eq!(hash1.hash, compute_file_hash(&disk[0].path).unwrap());

        // Second run with the fresh hints: classification identical, and the
        // reused hash must equal the real content hash (correctness of the
        // fast path — a wrong reuse would mark changed files unchanged).
        let (diff2, fresh2) = diff_files_with_hints(&disk, &db, false, &fresh).unwrap();
        assert_eq!(diff2.added.len(), 1, "still added (not in DB)");
        assert_eq!(
            fresh2
                .get("a.rs")
                .expect("second run must refresh the hint"),
            &hash1,
            "unchanged file → identical hint"
        );
    }

    /// A file modified after the hint was recorded must NOT reuse the stale
    /// hash (mtime differs → full re-hash → changed classification).
    #[test]
    fn diff_files_with_hints_falls_back_to_full_hash_on_change() {
        let tmp = TempDir::new().unwrap();
        let f = make_file(tmp.path(), "a.rs", "fn a() {}", Language::Rust);
        let disk = vec![f];
        let db: Vec<(String, String)> = vec![];

        let (_, fresh) = diff_files_with_hints(&disk, &db, false, &HashMap::new()).unwrap();

        // Modify the file after the hint snapshot.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&disk[0].path, "fn b() {}").unwrap();

        let (diff, fresh2) = diff_files_with_hints(&disk, &db, false, &fresh).unwrap();
        assert_eq!(diff.added.len(), 1, "modified file must be re-hashed");
        let new_hash = fresh2.get("a.rs").unwrap().hash.clone();
        assert_eq!(new_hash, compute_file_hash(&disk[0].path).unwrap());
        assert_ne!(
            new_hash,
            fresh.get("a.rs").unwrap().hash,
            "stale hint must not leak into the new hint"
        );
    }

    // --- diff_files: matching hash → unchanged ---

    #[test]
    fn diff_files_matching_hash_goes_to_unchanged() {
        let tmp = TempDir::new().unwrap();
        let f = make_file(tmp.path(), "a.rs", "fn a() {}", Language::Rust);
        let disk = vec![f];
        let db = vec![("a.rs".to_string(), hash_of(tmp.path(), "a.rs"))];

        let diff = diff_files(&disk, &db, false).unwrap();

        assert_eq!(
            diff.unchanged.len(),
            1,
            "matching hash → unchanged (BR-INDEX-001)"
        );
        assert!(diff.changed.is_empty());
        assert!(diff.added.is_empty());
        assert!(diff.deleted.is_empty());
    }

    // --- diff_files: different hash → changed ---

    #[test]
    fn diff_files_different_hash_goes_to_changed() {
        let tmp = TempDir::new().unwrap();
        let f = make_file(
            tmp.path(),
            "a.rs",
            "fn a() { /* modified */ }",
            Language::Rust,
        );
        let disk = vec![f];
        // DB hash is for the OLD content, so it differs from the current hash.
        let db = vec![("a.rs".to_string(), "0".repeat(64))];

        let diff = diff_files(&disk, &db, false).unwrap();

        assert_eq!(diff.changed.len(), 1, "different hash → changed");
        assert!(diff.unchanged.is_empty());
        assert!(diff.added.is_empty());
        assert!(diff.deleted.is_empty());
    }

    // --- diff_files: in DB not on disk → deleted ---

    #[test]
    fn diff_files_in_db_not_on_disk_goes_to_deleted() {
        let tmp = TempDir::new().unwrap();
        let f = make_file(tmp.path(), "a.rs", "fn a() {}", Language::Rust);
        let disk = vec![f];
        let db = vec![
            ("a.rs".to_string(), hash_of(tmp.path(), "a.rs")),
            ("deleted.rs".to_string(), "deadbeef".to_string()),
        ];

        let diff = diff_files(&disk, &db, false).unwrap();

        assert_eq!(
            diff.deleted.len(),
            1,
            "BR-INDEX-002: in DB not on disk → deleted"
        );
        assert_eq!(diff.deleted[0], "deleted.rs");
        assert_eq!(diff.unchanged.len(), 1);
    }

    // --- diff_files: force=true → all disk files in changed ---

    #[test]
    fn diff_files_force_puts_all_disk_files_in_changed() {
        let tmp = TempDir::new().unwrap();
        let f1 = make_file(tmp.path(), "a.rs", "fn a() {}", Language::Rust);
        let f2 = make_file(tmp.path(), "b.rs", "fn b() {}", Language::Rust);
        let disk = vec![f1, f2];
        // Even though hashes match, force=true must override.
        let db = vec![
            ("a.rs".to_string(), hash_of(tmp.path(), "a.rs")),
            ("b.rs".to_string(), hash_of(tmp.path(), "b.rs")),
        ];

        let diff = diff_files(&disk, &db, true).unwrap();

        assert_eq!(diff.changed.len(), 2, "BR-INDEX-003: force → all changed");
        assert!(
            diff.unchanged.is_empty(),
            "force must skip the unchanged bucket"
        );
        assert!(diff.added.is_empty());
        assert!(
            diff.deleted.is_empty(),
            "force does not affect deleted detection"
        );
    }

    #[test]
    fn diff_files_force_with_new_file_goes_to_changed() {
        let tmp = TempDir::new().unwrap();
        let f = make_file(tmp.path(), "new.rs", "fn new() {}", Language::Rust);
        let disk = vec![f];
        let db: Vec<(String, String)> = vec![];

        let diff = diff_files(&disk, &db, true).unwrap();

        // With force=true, even new files go to changed (not added).
        assert_eq!(diff.changed.len(), 1);
        assert!(diff.added.is_empty());
    }

    // --- diff_files: empty disk, non-empty DB → all deleted ---

    #[test]
    fn diff_files_empty_disk_nonempty_db_all_deleted() {
        let disk: Vec<FileInfo> = vec![];
        let db = vec![
            ("a.rs".to_string(), "hash_a".to_string()),
            ("b.rs".to_string(), "hash_b".to_string()),
            ("c.rs".to_string(), "hash_c".to_string()),
        ];

        let diff = diff_files(&disk, &db, false).unwrap();

        assert_eq!(diff.deleted.len(), 3, "all DB files should be deleted");
        let deleted_paths: Vec<&str> = diff.deleted.iter().map(|s| s.as_str()).collect();
        assert!(deleted_paths.contains(&"a.rs"));
        assert!(deleted_paths.contains(&"b.rs"));
        assert!(deleted_paths.contains(&"c.rs"));
        assert!(diff.changed.is_empty());
        assert!(diff.added.is_empty());
        assert!(diff.unchanged.is_empty());
    }

    // --- diff_files: empty disk + empty DB → empty diff ---

    #[test]
    fn diff_files_empty_disk_empty_db_empty_diff() {
        let disk: Vec<FileInfo> = vec![];
        let db: Vec<(String, String)> = vec![];

        let diff = diff_files(&disk, &db, false).unwrap();

        assert!(diff.is_empty());
        assert_eq!(diff.total(), 0);
    }

    // --- diff_files: mixed scenario ---

    #[test]
    fn diff_files_mixed_scenario() {
        let tmp = TempDir::new().unwrap();
        // a.rs: unchanged (hash matches DB)
        let a = make_file(tmp.path(), "a.rs", "fn a() {}", Language::Rust);
        // b.rs: changed (hash differs from DB)
        let b = make_file(tmp.path(), "b.rs", "fn b() { /* new */ }", Language::Rust);
        // c.rs: added (not in DB)
        let c = make_file(tmp.path(), "c.rs", "fn c() {}", Language::Rust);
        let disk = vec![a, b, c];
        let db = vec![
            ("a.rs".to_string(), hash_of(tmp.path(), "a.rs")), // matches
            ("b.rs".to_string(), "0".repeat(64)),              // differs
            // c.rs not in DB → added
            ("deleted.rs".to_string(), "old_hash".to_string()), // not on disk → deleted
        ];

        let diff = diff_files(&disk, &db, false).unwrap();

        assert_eq!(diff.unchanged.len(), 1, "a.rs unchanged");
        assert_eq!(diff.changed.len(), 1, "b.rs changed");
        assert_eq!(diff.added.len(), 1, "c.rs added");
        assert_eq!(diff.deleted.len(), 1, "deleted.rs deleted");

        let unchanged_paths: Vec<&str> = diff
            .unchanged
            .iter()
            .map(|f| f.relative_path.as_str())
            .collect();
        let changed_paths: Vec<&str> = diff
            .changed
            .iter()
            .map(|f| f.relative_path.as_str())
            .collect();
        let added_paths: Vec<&str> = diff
            .added
            .iter()
            .map(|f| f.relative_path.as_str())
            .collect();
        assert!(unchanged_paths.contains(&"a.rs"));
        assert!(changed_paths.contains(&"b.rs"));
        assert!(added_paths.contains(&"c.rs"));
        assert_eq!(diff.deleted[0], "deleted.rs");
    }

    // --- diff_files: returns error for missing file ---

    #[test]
    fn diff_files_returns_error_when_disk_file_disappears() {
        // A FileInfo whose path does not exist on disk should cause an error.
        let file = FileInfo {
            path: PathBuf::from("/nonexistent/missing.rs"),
            relative_path: "missing.rs".to_string(),
            language: Some(Language::Rust),
            size: 0,
        };
        let disk = vec![file];
        let db: Vec<(String, String)> = vec![];

        let result = diff_files(&disk, &db, false);
        assert!(result.is_err(), "missing disk file should error");
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    // --- diff_files: force=true still computes deleted ---

    #[test]
    fn diff_files_force_still_computes_deleted() {
        let tmp = TempDir::new().unwrap();
        let f = make_file(tmp.path(), "a.rs", "fn a() {}", Language::Rust);
        let disk = vec![f];
        let db = vec![
            ("a.rs".to_string(), hash_of(tmp.path(), "a.rs")),
            ("gone.rs".to_string(), "old".to_string()),
        ];

        let diff = diff_files(&disk, &db, true).unwrap();

        // a.rs goes to changed (force), gone.rs goes to deleted.
        assert_eq!(diff.changed.len(), 1);
        assert_eq!(diff.deleted.len(), 1);
        assert_eq!(diff.deleted[0], "gone.rs");
    }

    // --- diff_files: handles nested paths ---

    #[test]
    fn diff_files_handles_nested_paths() {
        let tmp = TempDir::new().unwrap();
        let f1 = make_file(tmp.path(), "src/main.rs", "fn main() {}", Language::Rust);
        let f2 = make_file(
            tmp.path(),
            "src/sub/mod.rs",
            "fn mod_fn() {}",
            Language::Rust,
        );
        let disk = vec![f1, f2];
        let db = vec![
            (
                "src/main.rs".to_string(),
                hash_of(tmp.path(), "src/main.rs"),
            ),
            // src/sub/mod.rs is new
        ];

        let diff = diff_files(&disk, &db, false).unwrap();

        assert_eq!(diff.unchanged.len(), 1);
        assert_eq!(diff.added.len(), 1);
        let added_paths: Vec<&str> = diff
            .added
            .iter()
            .map(|f| f.relative_path.as_str())
            .collect();
        assert!(added_paths.contains(&"src/sub/mod.rs"));
    }

    // --- diff_files: same path different project handled by caller ---

    #[test]
    fn diff_files_uses_relative_path_as_key() {
        // The DB hashes are scoped to a project by the caller (via
        // Repository::get_all_file_hashes(project)). diff_files itself only
        // compares paths, so the same relative path in two projects is handled
        // by calling diff_files twice with different db_hashes.
        let tmp = TempDir::new().unwrap();
        let f = make_file(tmp.path(), "main.rs", "fn main() {}", Language::Rust);
        let disk = vec![f];
        let db = vec![("main.rs".to_string(), hash_of(tmp.path(), "main.rs"))];

        let diff = diff_files(&disk, &db, false).unwrap();
        assert_eq!(diff.unchanged.len(), 1);
    }

    // --- diff_files: force=true with empty DB → all changed ---

    #[test]
    fn diff_files_force_with_empty_db_all_changed() {
        let tmp = TempDir::new().unwrap();
        let f1 = make_file(tmp.path(), "a.rs", "fn a() {}", Language::Rust);
        let f2 = make_file(tmp.path(), "b.rs", "fn b() {}", Language::Rust);
        let disk = vec![f1, f2];
        let db: Vec<(String, String)> = vec![];

        let diff = diff_files(&disk, &db, true).unwrap();

        assert_eq!(diff.changed.len(), 2);
        assert!(diff.added.is_empty(), "force overrides added → changed");
        assert!(diff.unchanged.is_empty());
        assert!(diff.deleted.is_empty());
    }
}
