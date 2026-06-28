// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `import` subcommand handler (H7, design.md D6).
//!
//! Decompresses a `codenexus.graph.zst` team artifact (produced by
//! [`crate::cli::export_cmd`]) into a LadybugDB database file. Optionally
//! triggers an incremental reindex of the local diff via `index_cmd` when
//! `--reindex` is given with `--path` and `--name`.
//!
//! # Fail-loud behavior (user rule 12)
//!
//! - Returns [`CliError::InvalidInput`] if the artifact's magic bytes don't
//!   match `CNXP`.
//! - Returns [`CliError::InvalidInput`] if the manifest's `format_version`
//!   differs from [`ARTIFACT_FORMAT_VERSION`] (forward-compatibility guard).
//! - Returns [`CliError::InvalidInput`] if `--reindex` is given without both
//!   `--path` and `--name`.
//!
//! # zstd CLI dependency
//!
//! Decompression shells out to the system `zstd` binary (see
//! [`crate::cli::export_cmd`] for the rationale — the `zstd` Rust crate
//! conflicts with lbug's statically-linked zstd at link time).

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::Serialize;

use super::args::ImportArgs;
use super::error::{CliError, Result};
use super::export_cmd::{ArtifactManifest, ARTIFACT_FORMAT_VERSION, ARTIFACT_MAGIC, ZSTD_BIN};

/// JSON-serializable import-command output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ImportOutput {
    /// Source artifact path that was read.
    pub artifact: String,
    /// Destination database path that was written.
    pub db: String,
    /// Decompressed DB file size in bytes.
    pub db_size: u64,
    /// Manifest from the artifact (for traceability).
    pub manifest: ArtifactManifest,
    /// Whether an incremental reindex was triggered.
    pub reindexed: bool,
}

/// Decompresses `input` (zstd-compressed bytes) via the system `zstd` CLI
/// and returns the decompressed bytes.
///
/// # Errors
///
/// Returns [`CliError::InvalidInput`] if the `zstd` binary is not on PATH
/// or exits non-zero. Returns [`CliError::Io`] for spawn/pipe failures.
fn zstd_decompress(input: &[u8]) -> Result<Vec<u8>> {
    let mut child = Command::new(ZSTD_BIN)
        .args(["-q", "-d", "-c"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CliError::InvalidInput(format!(
                    "zstd binary not found on PATH — install zstd to use export/import (H7). Error: {e}"
                ))
            } else {
                CliError::Io(e)
            }
        })?;
    {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            CliError::Io(std::io::Error::other("zstd stdin not captured"))
        })?;
        stdin.write_all(input)?;
        // stdin is dropped here, signaling EOF to zstd.
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::InvalidInput(format!(
            "zstd decompression failed (status {}): {}",
            output.status,
            stderr.trim()
        )));
    }
    Ok(output.stdout)
}

/// Runs the `import` subcommand.
///
/// Reads the artifact at `args.input`, validates the magic + manifest, and
/// writes the decompressed DB bytes to `args.db`. If `--reindex` is set,
/// triggers `index_cmd::run` with `--path` and `--name` for an incremental
/// reindex of the local diff.
///
/// # Errors
///
/// Returns [`CliError::InvalidInput`] for: bad magic, version mismatch,
/// missing artifact file, or `--reindex` without `--path`/`--name`.
/// Returns [`CliError::Io`] for filesystem failures.
/// Returns [`CliError::Index`] if the reindex step fails.
pub fn run(kit: &crate::kit::Kit, args: &ImportArgs) -> Result<()> {
    let input_path = Path::new(&args.input);
    if !input_path.exists() {
        return Err(CliError::InvalidInput(format!(
            "artifact path does not exist: {}",
            args.input
        )));
    }

    // Read the entire artifact into memory. Artifacts are typically small
    // (a few MB for a codebase), and we need random access to the header.
    let artifact_bytes = std::fs::read(input_path)?;
    if artifact_bytes.len() < 8 {
        return Err(CliError::InvalidInput(format!(
            "artifact too small ({} bytes) — expected at least 8-byte header",
            artifact_bytes.len()
        )));
    }
    if artifact_bytes[0..4] != ARTIFACT_MAGIC {
        return Err(CliError::InvalidInput(format!(
            "artifact magic mismatch — expected {:?}, got {:?}",
            std::str::from_utf8(&ARTIFACT_MAGIC),
            std::str::from_utf8(&artifact_bytes[0..4])
        )));
    }
    let manifest_len = u32::from_le_bytes([
        artifact_bytes[4],
        artifact_bytes[5],
        artifact_bytes[6],
        artifact_bytes[7],
    ]) as usize;
    let header_total = 8 + manifest_len;
    if artifact_bytes.len() < header_total {
        return Err(CliError::InvalidInput(format!(
            "artifact truncated — header says manifest is {} bytes but file only has {} bytes total",
            manifest_len,
            artifact_bytes.len()
        )));
    }
    let manifest: ArtifactManifest =
        serde_json::from_slice(&artifact_bytes[8..header_total])?;
    if manifest.format_version != ARTIFACT_FORMAT_VERSION {
        return Err(CliError::InvalidInput(format!(
            "artifact format version mismatch — expected {}, got {}",
            ARTIFACT_FORMAT_VERSION, manifest.format_version
        )));
    }

    // Decompress the zstd payload via the zstd CLI.
    let compressed_payload = &artifact_bytes[header_total..];
    let db_bytes = zstd_decompress(compressed_payload)?;

    let db_path = Path::new(&args.db);
    {
        let mut out = File::create(db_path)?;
        out.write_all(&db_bytes)?;
        out.flush()?;
    }

    let db_size = db_path.metadata()?.len();

    // Optionally trigger an incremental reindex.
    let mut reindexed = false;
    if args.reindex {
        let path = args
            .path
            .as_deref()
            .ok_or_else(|| CliError::InvalidInput("--reindex requires --path".into()))?;
        let name = args
            .name
            .as_deref()
            .ok_or_else(|| CliError::InvalidInput("--reindex requires --name".into()))?;
        let index_args = super::args::IndexArgs {
            path: path.to_string(),
            name: name.to_string(),
            db: args.db.clone(),
            force: false,
            lsp: false,
            embed: false,
            ram_first: false,
        };
        super::index_cmd::run(kit, &index_args)?;
        reindexed = true;
    }

    let output = ImportOutput {
        artifact: args.input.clone(),
        db: args.db.clone(),
        db_size,
        manifest,
        reindexed,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{ImportArgs, IndexArgs};
    use crate::cli::export_cmd::zstd_compress;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn make_args(input: &str, db: &str) -> ImportArgs {
        ImportArgs {
            input: input.to_string(),
            db: db.to_string(),
            reindex: false,
            path: None,
            name: None,
        }
    }

    /// Returns `true` if the `zstd` binary is available on PATH.
    fn zstd_available() -> bool {
        std::process::Command::new(ZSTD_BIN)
            .arg("--version")
            .output()
            .is_ok()
    }

    /// Skips the calling test if the `zstd` binary is not installed on PATH.
    fn skip_without_zstd() -> bool {
        if zstd_available() {
            false
        } else {
            eprintln!(
                "skipping test: '{}' binary not found on PATH",
                ZSTD_BIN
            );
            true
        }
    }

    /// Builds a tiny artifact on disk for testing. Returns the artifact path.
    fn build_test_artifact(dir: &Path, name: &str, payload: &[u8]) -> PathBuf {
        let artifact_path = dir.join(name);
        let manifest = ArtifactManifest {
            format_version: ARTIFACT_FORMAT_VERSION.into(),
            codenexus_version: env!("CARGO_PKG_VERSION").into(),
            exported_at: 1_700_000_000,
            source_db_path: "/fake/source.lbug".into(),
            project: Some("demo".into()),
            original_size: payload.len() as u64,
        };
        let manifest_json = serde_json::to_vec(&manifest).unwrap();
        let compressed = zstd_compress(payload).expect("zstd_compress");
        let mut out = File::create(&artifact_path).unwrap();
        out.write_all(&ARTIFACT_MAGIC).unwrap();
        let len = u32::try_from(manifest_json.len()).unwrap();
        out.write_all(&len.to_le_bytes()).unwrap();
        out.write_all(&manifest_json).unwrap();
        out.write_all(&compressed).unwrap();
        artifact_path
    }

    // --- ImportOutput ---

    #[test]
    fn import_output_serializes_to_json() {
        let m = ArtifactManifest {
            format_version: "1.0".into(),
            codenexus_version: "0.1.0".into(),
            exported_at: 1,
            source_db_path: "/x.lbug".into(),
            project: None,
            original_size: 10,
        };
        let out = ImportOutput {
            artifact: "/x.zst".into(),
            db: "/y.lbug".into(),
            db_size: 100,
            manifest: m,
            reindexed: false,
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"artifact\":\"/x.zst\""));
        assert!(json.contains("\"db\":\"/y.lbug\""));
        assert!(json.contains("\"reindexed\":false"));
    }

    // --- run() success ---

    #[test]
    fn run_import_decompresses_artifact_to_db_file() {
        if skip_without_zstd() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let payload = b"hello codenexus import";
        let artifact = build_test_artifact(dir.path(), "a.zst", payload);
        let db_path = dir.path().join("out.lbug");

        let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from("/tmp/_unused_imp1.lbug")))
            .expect("build_kit");
        let args = make_args(artifact.to_str().unwrap(), db_path.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "import should succeed: {:?}", result.err());

        let db_bytes = std::fs::read(&db_path).unwrap();
        assert_eq!(db_bytes, payload);
    }

    #[test]
    fn run_import_preserves_manifest_in_output() {
        if skip_without_zstd() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let artifact = build_test_artifact(dir.path(), "b.zst", b"db-bytes");
        let db_path = dir.path().join("out2.lbug");

        let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from("/tmp/_unused_imp2.lbug")))
            .expect("build_kit");
        let args = make_args(artifact.to_str().unwrap(), db_path.to_str().unwrap());
        // We can't easily capture stdout, so verify the DB file landed and
        // has the right content (which only happens after the manifest was
        // parsed successfully).
        let result = run(&kit, &args);
        assert!(result.is_ok(), "import should succeed: {:?}", result.err());
        assert_eq!(std::fs::read(&db_path).unwrap(), b"db-bytes");
    }

    // --- run() error cases ---

    #[test]
    fn run_import_missing_artifact_returns_invalid_input() {
        let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from("/tmp/_unused_imp3.lbug")))
            .expect("build_kit");
        let args = make_args("/nonexistent/artifact.zst", "/tmp/out.lbug");
        let err = run(&kit, &args).expect_err("missing artifact should error");
        assert_eq!(err.exit_code(), 1);
        match err {
            CliError::InvalidInput(msg) => assert!(msg.contains("does not exist")),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn run_import_bad_magic_returns_invalid_input() {
        let dir = TempDir::new().unwrap();
        let bad_artifact = dir.path().join("bad.zst");
        std::fs::write(&bad_artifact, b"XXXX\xff\xff\xff\xffrest").unwrap();

        let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from("/tmp/_unused_imp4.lbug")))
            .expect("build_kit");
        let args = make_args(bad_artifact.to_str().unwrap(), "/tmp/out.lbug");
        let err = run(&kit, &args).expect_err("bad magic should error");
        assert_eq!(err.exit_code(), 1);
        match err {
            CliError::InvalidInput(msg) => assert!(msg.contains("magic")),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn run_import_truncated_header_returns_invalid_input() {
        let dir = TempDir::new().unwrap();
        let tiny = dir.path().join("tiny.zst");
        // 3 bytes — less than the 8-byte header minimum.
        std::fs::write(&tiny, b"CNX").unwrap();

        let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from("/tmp/_unused_imp5.lbug")))
            .expect("build_kit");
        let args = make_args(tiny.to_str().unwrap(), "/tmp/out.lbug");
        let err = run(&kit, &args).expect_err("truncated header should error");
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn run_import_version_mismatch_returns_invalid_input() {
        let dir = TempDir::new().unwrap();
        let artifact = dir.path().join("v2.zst");
        // Build an artifact with format_version "2.0" (mismatch).
        let manifest = ArtifactManifest {
            format_version: "2.0".into(),
            codenexus_version: "0.1.0".into(),
            exported_at: 0,
            source_db_path: "/x.lbug".into(),
            project: None,
            original_size: 0,
        };
        let manifest_json = serde_json::to_vec(&manifest).unwrap();
        let mut out = File::create(&artifact).unwrap();
        out.write_all(&ARTIFACT_MAGIC).unwrap();
        let len = u32::try_from(manifest_json.len()).unwrap();
        out.write_all(&len.to_le_bytes()).unwrap();
        out.write_all(&manifest_json).unwrap();
        // Empty compressed payload (no DB content needed for this error path).
        out.write_all(b"").unwrap();

        let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from("/tmp/_unused_imp6.lbug")))
            .expect("build_kit");
        let args = make_args(artifact.to_str().unwrap(), "/tmp/out.lbug");
        let err = run(&kit, &args).expect_err("version mismatch should error");
        assert_eq!(err.exit_code(), 1);
        match err {
            CliError::InvalidInput(msg) => assert!(msg.contains("version")),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn run_import_reindex_without_path_returns_invalid_input() {
        if skip_without_zstd() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let artifact = build_test_artifact(dir.path(), "c.zst", b"db");
        let db_path = dir.path().join("out3.lbug");

        let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from("/tmp/_unused_imp7.lbug")))
            .expect("build_kit");
        let args = ImportArgs {
            input: artifact.to_string_lossy().into_owned(),
            db: db_path.to_string_lossy().into_owned(),
            reindex: true,
            path: None,
            name: Some("demo".into()),
        };
        let err = run(&kit, &args).expect_err("--reindex without --path should error");
        assert_eq!(err.exit_code(), 1);
        match err {
            CliError::InvalidInput(msg) => assert!(msg.contains("--path")),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn run_import_reindex_without_name_returns_invalid_input() {
        if skip_without_zstd() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let artifact = build_test_artifact(dir.path(), "d.zst", b"db");
        let db_path = dir.path().join("out4.lbug");

        let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from("/tmp/_unused_imp8.lbug")))
            .expect("build_kit");
        let args = ImportArgs {
            input: artifact.to_string_lossy().into_owned(),
            db: db_path.to_string_lossy().into_owned(),
            reindex: true,
            path: Some("/tmp".into()),
            name: None,
        };
        let err = run(&kit, &args).expect_err("--reindex without --name should error");
        assert_eq!(err.exit_code(), 1);
        match err {
            CliError::InvalidInput(msg) => assert!(msg.contains("--name")),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    // --- End-to-end: export then import round-trip ---

    #[test]
    fn export_then_import_round_trips_db_bytes() {
        if skip_without_zstd() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let src_db = dir.path().join("src.lbug");
        // Write a fake "DB" file (we don't need a real LadybugDB for this
        // round-trip test — export/import treat it as opaque bytes).
        let original_bytes = b"fake-db-content-12345";
        std::fs::write(&src_db, original_bytes).unwrap();

        // Export.
        let artifact_path = dir.path().join("out.zst");
        let export_args = crate::cli::args::ExportArgs {
            output: artifact_path.to_string_lossy().into_owned(),
            db: src_db.to_string_lossy().into_owned(),
            project: Some("demo".into()),
        };
        let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from("/tmp/_unused_e2e.lbug")))
            .expect("build_kit");
        let result = crate::cli::export_cmd::run(&kit, &export_args);
        assert!(result.is_ok(), "export should succeed: {:?}", result.err());
        assert!(artifact_path.exists());

        // Import.
        let dst_db = dir.path().join("dst.lbug");
        let import_args = make_args(
            artifact_path.to_str().unwrap(),
            dst_db.to_str().unwrap(),
        );
        let result = run(&kit, &import_args);
        assert!(result.is_ok(), "import should succeed: {:?}", result.err());

        // Verify the imported DB matches the original.
        let imported_bytes = std::fs::read(&dst_db).unwrap();
        assert_eq!(imported_bytes, original_bytes);
    }

    // --- IndexArgs sanity (used internally by import --reindex) ---

    #[test]
    fn index_args_is_constructible_for_reindex() {
        let _ = IndexArgs {
            path: "/r".into(),
            name: "d".into(),
            db: "./x.lbug".into(),
            force: false,
            lsp: false,
            embed: false,
            ram_first: false,
        };
    }
}
