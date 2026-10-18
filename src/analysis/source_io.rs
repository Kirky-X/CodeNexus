// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Source-file reads for analyzers that fall back to disk when the indexed
//! `content` column is empty (the indexer does not persist function bodies).
//!
//! Shared by [`ComplexityAnalyzer`](super::complexity::ComplexityAnalyzer) and
//! [`CrossServiceLinker`](super::cross_service::CrossServiceLinker) /
//! [`CrossServiceDetector`](super::cross_service::CrossServiceDetector).
//!
//! All reads are path-traversal protected: the target is resolved under the
//! project's canonical `rootPath` and must stay inside it, so a hostile
//! stored `filePath` (`../…`) cannot escape the indexed workspace.

use std::path::Path;

/// Reads the full source file at `canonical_root/file_path` with path-traversal
/// protection. `canonical_root` must already be canonicalized (e.g. via
/// `Path::canonicalize`). Returns `None` if the file cannot be read or the
/// resolved path escapes `canonical_root`.
///
/// The joined target is canonicalized and must start with `canonical_root`.
/// This blocks `../` sequences and symlink escapes. Leading `/` in `file_path`
/// is stripped so stored absolute-looking filePaths are treated as relative to
/// `canonical_root`.
///
/// The file is opened via `File::open` on the canonicalized path and read from
/// the handle, narrowing the TOCTOU window between canonicalize and read
/// compared to `fs::read_to_string` (which re-resolves the path internally).
pub(crate) fn read_source_file_canonical(canonical_root: &Path, file_path: &str) -> Option<String> {
    use std::fs::File;
    use std::io::Read;

    let relative = file_path.trim_start_matches('/');
    let canonical_target = canonical_root.join(relative).canonicalize().ok()?;
    if !canonical_target.starts_with(canonical_root) {
        return None;
    }
    let mut file = File::open(&canonical_target).ok()?;
    let mut content = String::new();
    file.read_to_string(&mut content).ok()?;
    Some(content)
}

/// Slices `content` by 1-based line range `[start_line, end_line]` (inclusive).
/// Returns `None` for invalid ranges or `start_line` beyond the file length.
/// `end_line` is clamped to the actual line count.
///
/// Uses lazy `skip().take()` iteration so only the requested lines are
/// materialized, avoiding a full `Vec<&str>` allocation for large files.
pub(crate) fn slice_lines(content: &str, start_line: u32, end_line: u32) -> Option<String> {
    if start_line == 0 || end_line == 0 || start_line > end_line {
        return None;
    }
    let start = (start_line - 1) as usize;
    let count = (end_line - start_line + 1) as usize;
    let selected: Vec<&str> = content.lines().skip(start).take(count).collect();
    if selected.is_empty() {
        return None;
    }
    Some(selected.join("\n"))
}

/// Reads lines `[start_line, end_line]` (1-based, inclusive) from
/// `canonical_root/file_path` (path-traversal protected via
/// [`read_source_file_canonical`]).
///
/// Callers reading many files from the same root should canonicalize the root
/// once and keep calling this function (the canonicalize cost is per file).
pub(crate) fn read_source_slice_canonical(
    canonical_root: &Path,
    file_path: &str,
    start_line: u32,
    end_line: u32,
) -> Option<String> {
    if start_line == 0 || end_line == 0 {
        return None;
    }
    let content = read_source_file_canonical(canonical_root, file_path)?;
    slice_lines(&content, start_line, end_line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn read_source_file_canonical_rejects_traversal() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("lib.rs"), "safe\n").unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert!(read_source_file_canonical(&root, "../escape.rs").is_none());
        assert!(read_source_file_canonical(&root, "lib.rs").is_some());
    }

    #[test]
    fn slice_lines_returns_expected_range_and_clamps() {
        assert_eq!(slice_lines("a\nb\nc\nd", 2, 3).unwrap(), "b\nc");
        assert_eq!(slice_lines("a\nb", 1, 99).unwrap(), "a\nb");
        assert!(slice_lines("a\nb", 0, 1).is_none());
        assert!(slice_lines("a\nb", 5, 6).is_none());
    }
}
