// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Source-file reads that slice code text from disk by 1-based line range.
//!
//! Shared by the analyzers ([`ComplexityAnalyzer`], [`CrossServiceLinker`] /
//! [`CrossServiceDetector`], re-exported via `crate::analysis`) and by the
//! `service::source` symbol-source reader ([`crate::service::source`]).
//!
//! All reads are path-traversal protected: the target is resolved under the
//! project's canonical `rootPath` and must stay inside it, so a hostile
//! stored `filePath` (`../…`) cannot escape the indexed workspace.

use std::path::Path;

use crate::index::hash::MAX_FILE_SIZE;

/// Reads the full source file at `canonical_root/file_path` with path-traversal
/// protection. `canonical_root` must already be canonicalized (e.g. via
/// `Path::canonicalize`). Returns `None` if the file cannot be read, the
/// resolved path escapes `canonical_root`, or the file exceeds
/// [`MAX_FILE_SIZE`].
///
/// The joined target is canonicalized and must start with `canonical_root`.
/// This blocks `../` sequences and symlink escapes. Leading `/` in `file_path`
/// is stripped so stored absolute-looking filePaths are treated as relative to
/// `canonical_root`.
///
/// The size cap is checked on the metadata **before** any read (same constant
/// as the hashing path): the analysis-side callers of this helper have no
/// hashing stage to bound them, and an unbounded slurp is an OOM hazard.
///
/// The file is opened via `File::open` on the canonicalized path; the size
/// cap is checked on the OPENED HANDLE (`handle.metadata()` = fstat on the
/// same object) and the read is bounded with `take`, so a concurrent
/// replace/grow between stat and read can neither slip past the cap nor blow
/// memory — the earlier `symlink_metadata`-then-`fs::read` shape left that
/// window open.
pub(crate) fn read_source_file_canonical(canonical_root: &Path, file_path: &str) -> Option<String> {
    use std::fs::File;
    use std::io::Read;

    let relative = file_path.trim_start_matches('/');
    let canonical_target = canonical_root.join(relative).canonicalize().ok()?;
    if !canonical_target.starts_with(canonical_root) {
        return None;
    }
    // 与 hash 路径同源的上限：超限文件在读取前拒绝，不依赖 hash 阶段兜底。
    // 上限检查落在已打开的句柄上（fstat 同一对象），消除 stat/read 竞态窗口。
    let file = File::open(&canonical_target).ok()?;
    if file.metadata().ok()?.len() > MAX_FILE_SIZE {
        return None;
    }
    let mut content = String::new();
    file.take(MAX_FILE_SIZE).read_to_string(&mut content).ok()?;
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

    /// 与 hash 路径同源的 10MB 上限必须在读取**前**生效：analysis 侧调用方
    /// 没有 hash 阶段兜底，超限文件不得整体读进内存。稀疏文件构造超限长度
    /// （与 hash.rs 的同款手法），不占真实磁盘。
    #[test]
    fn read_source_file_canonical_rejects_oversized_file_before_reading() {
        use std::io::{Seek, Write};

        let dir = TempDir::new().unwrap();
        let oversized = dir.path().join("huge.rs");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&oversized)
            .unwrap();
        file.seek(std::io::SeekFrom::Start(MAX_FILE_SIZE + 1))
            .unwrap();
        file.write_all(b"x").unwrap();
        drop(file);
        assert!(
            std::fs::symlink_metadata(&oversized).unwrap().len() > MAX_FILE_SIZE,
            "sparse file must report an over-limit logical size"
        );

        let root = dir.path().canonicalize().unwrap();
        assert!(
            read_source_file_canonical(&root, "huge.rs").is_none(),
            "oversized file must be rejected before any read"
        );
        assert!(
            read_source_slice_canonical(&root, "huge.rs", 1, 2).is_none(),
            "slice path must reject before reading too"
        );
    }

    #[test]
    fn slice_lines_returns_expected_range_and_clamps() {
        assert_eq!(slice_lines("a\nb\nc\nd", 2, 3).unwrap(), "b\nc");
        assert_eq!(slice_lines("a\nb", 1, 99).unwrap(), "a\nb");
        assert!(slice_lines("a\nb", 0, 1).is_none());
        assert!(slice_lines("a\nb", 5, 6).is_none());
    }
}
