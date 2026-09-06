// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Atomic delivery: stage in the target directory, then rename.
//!
//! A failed write never touches the previous artifact — the staging file is
//! removed and the old bytes stay on disk. Successful deliveries return a
//! [`HashInfo`] receipt (BLAKE3 per ADR-009) so callers can pin what they
//! shipped.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::diagnostics::HashInfo;

/// Internal: staging file path for `target` (exposed for failure-injection
/// tests).
fn staging_path(target: &Path) -> PathBuf {
    let name = target.file_name().map_or_else(
        || "artifact".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    target
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!(".cnx-stage-{}-{name}", std::process::id()))
}

/// Writes `bytes` to `target` atomically.
///
/// # Errors
///
/// Returns the underlying [`std::io::Error`] when staging, writing, syncing,
/// or renaming fails; in every failure case the staging file is removed and
/// a pre-existing `target` is left untouched.
pub fn write_atomically(target: &Path, bytes: &[u8]) -> Result<HashInfo, std::io::Error> {
    let staging = staging_path(target);
    let write_result = (|| {
        let mut file = std::fs::File::create(&staging)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&staging, target)?;
        Ok(())
    })();
    if let Err(err) = write_result {
        let _ = std::fs::remove_file(&staging);
        return Err(err);
    }
    Ok(HashInfo {
        algorithm: "blake3".to_string(),
        hash: blake3::hash(bytes).to_string(),
        bytes: bytes.len() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_new_file_with_hash() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("out.html");
        let info = write_atomically(&target, b"hello diagram").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"hello diagram");
        assert_eq!(info.bytes, 13);
        assert_eq!(info.algorithm, "blake3");
        assert_eq!(info.hash, blake3::hash(b"hello diagram").to_string());
    }

    #[test]
    fn overwrite_replaces_previous_content_atomically() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("out.html");
        write_atomically(&target, b"old-contents").unwrap();
        let info = write_atomically(&target, b"new").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(info.bytes, 3);
        // No staging leftovers.
        assert!(!staging_path(&target).exists());
    }

    #[test]
    fn failure_keeps_previous_artifact_and_cleans_staging() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("out.html");
        write_atomically(&target, b"previous-good").unwrap();
        // Occupy the staging path with a directory so File::create fails.
        std::fs::create_dir(staging_path(&target)).unwrap();
        let err = write_atomically(&target, b"never-lands");
        assert!(err.is_err(), "expected failure");
        assert_eq!(std::fs::read(&target).unwrap(), b"previous-good");
        std::fs::remove_dir(staging_path(&target)).unwrap();
    }

    #[test]
    fn failure_in_missing_directory_does_not_create_target() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("missing-dir").join("out.html");
        assert!(write_atomically(&target, b"x").is_err());
        assert!(!target.exists());
    }
}
