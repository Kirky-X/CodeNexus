// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `export` subcommand handler (H7, design.md D6).
//!
//! Compresses a LadybugDB database file to a zstd-compressed team artifact
//! (`codenexus.graph.zst`). The artifact carries a JSON manifest header so
//! `import` can verify codenexus version and original DB path.
//!
//! # Artifact format
//!
//! ```text
//! [4 bytes:  magic "CNXP"]
//! [4 bytes:  manifest_json_len (u32 little-endian)]
//! [N bytes:  JSON manifest bytes]
//! [rest:     zstd-compressed DB file bytes]
//! ```
//!
//! The manifest records `format_version`, `codenexus_version`,
//! `exported_at` (unix seconds), `source_db_path`, optional `project`,
//! and `original_size` (bytes). `import` reads the manifest first to
//! fail loudly on version mismatch before decompressing.
//!
//! # zstd CLI dependency
//!
//! Compression is performed by shelling out to the system `zstd` binary
//! (`std::process::Command`) rather than the `zstd` Rust crate. The crate
//! pulls in `zstd_sys` which duplicates `ZSTD_*` symbols that lbug already
//! statically links, causing link-time failures. The artifact bytes are
//! still standard zstd-compressed data (compatible with `zstd -d`).

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::args::ExportArgs;
use super::error::{CliError, Result};

/// Artifact magic bytes — "CNXP" (CodeNexus eXPort).
pub const ARTIFACT_MAGIC: [u8; 4] = *b"CNXP";

/// Current artifact format version.
pub const ARTIFACT_FORMAT_VERSION: &str = "1.0";

/// Name of the zstd CLI binary invoked for compression.
pub const ZSTD_BIN: &str = "zstd";

/// zstd compression level used by `export`. 19 is the max standard level
/// (highest ratio); `--ultra` is required for levels 20+ and is not used
/// here to keep the binary broadly compatible.
pub const ZSTD_LEVEL: &str = "19";

/// JSON manifest embedded in the artifact header.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactManifest {
    /// Artifact format version (semver string).
    pub format_version: String,
    /// Codenexus version that produced this artifact (`CARGO_PKG_VERSION`).
    pub codenexus_version: String,
    /// Unix timestamp (seconds) when the export ran.
    pub exported_at: u64,
    /// Original database path that was exported.
    pub source_db_path: String,
    /// Optional project name (for multi-project isolation).
    pub project: Option<String>,
    /// Original DB file size in bytes (sanity check on import).
    pub original_size: u64,
}

/// JSON-serializable export-command output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ExportOutput {
    /// Artifact path written.
    pub artifact: String,
    /// Artifact size in bytes (compressed).
    pub artifact_size: u64,
    /// Original DB file size in bytes.
    pub original_size: u64,
    /// Codenexus version that produced the artifact.
    pub codenexus_version: String,
    /// Unix timestamp (seconds) when the export ran.
    pub exported_at: u64,
}

/// Compresses `input` bytes via the system `zstd` CLI and returns the
/// compressed bytes.
///
/// # Errors
///
/// Returns [`CliError::InvalidInput`] if the `zstd` binary is not on PATH.
/// Returns [`CliError::Io`] if spawning or piping fails. Returns
/// [`CliError::InvalidInput`] if `zstd` exits non-zero.
pub(crate) fn zstd_compress(input: &[u8]) -> Result<Vec<u8>> {
    let mut child = Command::new(ZSTD_BIN)
        .args(["-q", "--ultra", format!("-{ZSTD_LEVEL}").as_str(), "-c"])
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
            "zstd compression failed (status {}): {}",
            output.status,
            stderr.trim()
        )));
    }
    Ok(output.stdout)
}

/// Runs the `export` subcommand.
///
/// Reads the LadybugDB file at `args.db`, compresses it with zstd, prepends
/// a manifest header, and writes the result to `args.output`. Prints a JSON
/// summary to stdout.
///
/// # Errors
///
/// Returns [`CliError::InvalidInput`] if the source DB path does not exist
/// or the `zstd` binary is missing.
/// Returns [`CliError::Io`] for filesystem read/write failures.
/// Returns [`CliError::Json`] for manifest serialization failures.
pub fn run(_kit: &crate::kit::Kit, args: &ExportArgs) -> Result<()> {
    let db_path = Path::new(&args.db);
    if !db_path.exists() {
        return Err(CliError::InvalidInput(format!(
            "database path does not exist: {}",
            args.db
        )));
    }

    let original_bytes = std::fs::read(db_path)?;
    let original_size = u64::try_from(original_bytes.len()).unwrap_or(0);
    let exported_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let manifest = ArtifactManifest {
        format_version: ARTIFACT_FORMAT_VERSION.to_string(),
        codenexus_version: env!("CARGO_PKG_VERSION").to_string(),
        exported_at,
        source_db_path: args.db.clone(),
        project: args.project.clone(),
        original_size,
    };
    let manifest_json = serde_json::to_vec(&manifest)?;

    let compressed = zstd_compress(&original_bytes)?;

    // Build the artifact: magic | manifest_len (u32 LE) | manifest | zstd(db).
    let output_path = Path::new(&args.output);
    {
        let mut out = File::create(output_path)?;
        out.write_all(&ARTIFACT_MAGIC)?;
        let manifest_len = u32::try_from(manifest_json.len()).map_err(|_| {
            CliError::InvalidInput(format!(
                "manifest too large ({} bytes) — exceeds u32::MAX",
                manifest_json.len()
            ))
        })?;
        out.write_all(&manifest_len.to_le_bytes())?;
        out.write_all(&manifest_json)?;
        out.write_all(&compressed)?;
        out.flush()?;
    }

    let artifact_size = output_path.metadata()?.len();
    let output = ExportOutput {
        artifact: args.output.clone(),
        artifact_size,
        original_size,
        codenexus_version: env!("CARGO_PKG_VERSION").to_string(),
        exported_at,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::ExportArgs;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cli_export_testdb.lbug");
        std::mem::forget(dir);
        path
    }

    /// Builds a Kit backed by an on-disk database at `db` (forces schema init
    /// so the file is non-empty).
    fn build_kit_for_db(db: &Path) -> crate::kit::Kit {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        build_kit(&config).expect("build_kit")
    }

    fn make_args(db: &str, output: &str) -> ExportArgs {
        ExportArgs {
            output: output.to_string(),
            db: db.to_string(),
            project: Some("demo".to_string()),
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
    /// Call as `if skip_without_zstd() { return; };` at the top of tests that
    /// need to spawn `zstd`.
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

    // --- ArtifactManifest ---

    #[test]
    fn manifest_serializes_to_json() {
        let m = ArtifactManifest {
            format_version: "1.0".into(),
            codenexus_version: "0.1.0".into(),
            exported_at: 1_700_000_000,
            source_db_path: "./codenexus.lbug".into(),
            project: Some("demo".into()),
            original_size: 4096,
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"format_version\":\"1.0\""));
        assert!(json.contains("\"original_size\":4096"));
    }

    #[test]
    fn manifest_round_trips_through_json() {
        let m = ArtifactManifest {
            format_version: "1.0".into(),
            codenexus_version: "0.1.0".into(),
            exported_at: 123,
            source_db_path: "/tmp/x.lbug".into(),
            project: None,
            original_size: 100,
        };
        let s = serde_json::to_string(&m).unwrap();
        let back: ArtifactManifest = serde_json::from_str(&s).unwrap();
        assert_eq!(m, back);
    }

    // --- ExportOutput ---

    #[test]
    fn export_output_serializes_to_json() {
        let out = ExportOutput {
            artifact: "/tmp/o.zst".into(),
            artifact_size: 100,
            original_size: 1000,
            codenexus_version: "0.1.0".into(),
            exported_at: 99,
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"artifact\":\"/tmp/o.zst\""));
        assert!(json.contains("\"artifact_size\":100"));
    }

    // --- zstd_compress unit ---

    #[test]
    fn zstd_compress_round_trips_with_zstd_cli() {
        if skip_without_zstd() {
            return;
        }
        let payload = b"hello codenexus export round trip";
        let compressed = zstd_compress(payload).expect("zstd_compress");
        assert!(!compressed.is_empty(), "compressed output should be non-empty");

        // Decompress via the same zstd CLI to verify round-trip.
        let mut child = std::process::Command::new(ZSTD_BIN)
            .args(["-q", "-d", "-c"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn zstd -d");
        {
            let mut stdin = child.stdin.take().expect("stdin");
            stdin.write_all(&compressed).expect("write stdin");
        }
        let out = child.wait_with_output().expect("wait");
        assert!(out.status.success(), "zstd -d failed: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(out.stdout, payload);
    }

    // --- run() success ---

    #[test]
    fn run_export_writes_artifact_with_magic_and_manifest() {
        if skip_without_zstd() {
            return;
        }
        let db = fresh_db_path();
        let _kit = build_kit_for_db(&db);
        // The Kit is dropped here, closing the DB so the file is readable.

        let dir = TempDir::new().unwrap();
        let out_path = dir.path().join("out.zst");
        let args = make_args(db.to_str().unwrap(), out_path.to_str().unwrap());
        let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from("/tmp/_unused.lbug")))
            .expect("build_kit");
        let result = run(&kit, &args);
        assert!(result.is_ok(), "export should succeed: {:?}", result.err());

        // Verify the artifact starts with magic + manifest header.
        let bytes = std::fs::read(&out_path).unwrap();
        assert!(bytes.len() > 12, "artifact should have header + body");
        assert_eq!(&bytes[0..4], &ARTIFACT_MAGIC);
        let manifest_len = u32::from_le_bytes([
            bytes[4], bytes[5], bytes[6], bytes[7],
        ]);
        let manifest_json = &bytes[8..8 + manifest_len as usize];
        let manifest: ArtifactManifest = serde_json::from_slice(manifest_json).unwrap();
        assert_eq!(manifest.format_version, ARTIFACT_FORMAT_VERSION);
        assert_eq!(manifest.codenexus_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(manifest.source_db_path, db.to_str().unwrap());
        assert_eq!(manifest.project.as_deref(), Some("demo"));
    }

    #[test]
    fn run_export_writes_nonempty_artifact() {
        if skip_without_zstd() {
            return;
        }
        let db = fresh_db_path();
        let _kit = build_kit_for_db(&db);

        let dir = TempDir::new().unwrap();
        let out_path = dir.path().join("out2.zst");
        let args = make_args(db.to_str().unwrap(), out_path.to_str().unwrap());
        let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from("/tmp/_unused2.lbug")))
            .expect("build_kit");
        let result = run(&kit, &args);
        assert!(result.is_ok(), "export should succeed: {:?}", result.err());
        assert!(out_path.exists(), "artifact file should exist");
        let size = out_path.metadata().unwrap().len();
        assert!(size > 0, "artifact should be non-empty");
    }

    // --- run() error cases ---

    #[test]
    fn run_export_missing_db_returns_invalid_input() {
        let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from("/tmp/_unused3.lbug")))
            .expect("build_kit");
        let args = make_args("/nonexistent/path/xyz.lbug", "/tmp/out.zst");
        let err = run(&kit, &args).expect_err("missing db should error");
        assert_eq!(err.exit_code(), 2, "InvalidInput → exit 2");
        match err {
            CliError::InvalidInput(msg) => {
                assert!(msg.contains("does not exist"), "got: {msg}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    // --- ARTIFACT_MAGIC ---

    #[test]
    fn artifact_magic_is_cnxp() {
        assert_eq!(&ARTIFACT_MAGIC, b"CNXP");
    }

    #[test]
    fn artifact_format_version_is_1_0() {
        assert_eq!(ARTIFACT_FORMAT_VERSION, "1.0");
    }

    /// Sanity: writing a tiny file through the same zstd CLI pipeline produces
    /// a decompressible artifact. This catches regressions in the header
    /// layout without depending on LadybugDB.
    #[test]
    fn header_layout_round_trips_with_arbitrary_bytes() {
        if skip_without_zstd() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src.bin");
        let artifact = dir.path().join("a.zst");
        // Write a tiny "DB" file.
        let payload = b"hello codenexus export";
        std::fs::write(&src, payload).unwrap();

        let manifest = ArtifactManifest {
            format_version: ARTIFACT_FORMAT_VERSION.into(),
            codenexus_version: env!("CARGO_PKG_VERSION").into(),
            exported_at: 0,
            source_db_path: src.to_string_lossy().into_owned(),
            project: None,
            original_size: payload.len() as u64,
        };
        let manifest_json = serde_json::to_vec(&manifest).unwrap();

        let compressed = zstd_compress(payload).expect("zstd_compress");

        let mut out = File::create(&artifact).unwrap();
        out.write_all(&ARTIFACT_MAGIC).unwrap();
        let len = u32::try_from(manifest_json.len()).unwrap();
        out.write_all(&len.to_le_bytes()).unwrap();
        out.write_all(&manifest_json).unwrap();
        out.write_all(&compressed).unwrap();
        drop(out);

        // Read back.
        let bytes = std::fs::read(&artifact).unwrap();
        assert_eq!(&bytes[0..4], &ARTIFACT_MAGIC);
        let mlen = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        let _m: ArtifactManifest =
            serde_json::from_slice(&bytes[8..8 + mlen]).unwrap();
        let compressed_payload = &bytes[8 + mlen..];

        // Decompress via zstd CLI.
        let mut child = std::process::Command::new(ZSTD_BIN)
            .args(["-q", "-d", "-c"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn zstd -d");
        {
            let mut stdin = child.stdin.take().expect("stdin");
            stdin.write_all(compressed_payload).unwrap();
        }
        let out = child.wait_with_output().expect("wait");
        assert!(out.status.success());
        assert_eq!(out.stdout, payload);
    }
}
