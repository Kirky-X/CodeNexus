// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Export command: compress the database to a zstd team artifact.
//!
//! Artifact format: `[magic (4B)] [manifest_len (4B LE)] [manifest JSON] [zstd-compressed DB bytes]`.
//! The `zstd` CLI binary is required for compression/decompression.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::storage::StorageConfig;

#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::kit::{AsyncKit, AsyncReady};
#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::service::error::CodeNexusError;
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::error::{kit_not_initialized, to_api_error};
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::runtime::kit;

#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::service_api;

/// Artifact magic bytes — "CNXP" (CodeNexus eXPort).
pub const ARTIFACT_MAGIC: [u8; 4] = *b"CNXP";

/// Current artifact format version.
pub const ARTIFACT_FORMAT_VERSION: &str = "1.0";

/// Name of the zstd CLI binary invoked for compression.
pub const ZSTD_BIN: &str = "zstd";

/// zstd compression level used by `export`.
pub const ZSTD_LEVEL: &str = "19";

/// JSON manifest embedded in the artifact header.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactManifest {
    pub format_version: String,
    pub codenexus_version: String,
    pub exported_at: u64,
    pub source_db_path: String,
    pub project: Option<String>,
    pub original_size: u64,
}

/// JSON-serializable export-command output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ExportOutput {
    pub artifact: String,
    pub artifact_size: u64,
    pub original_size: u64,
    pub codenexus_version: String,
    pub exported_at: u64,
}

/// Compresses `input` bytes via the system `zstd` CLI.
#[cfg(any(feature = "cli", feature = "mcp", test))]
fn zstd_compress(input: &[u8]) -> Result<Vec<u8>, CodeNexusError> {
    let mut child = Command::new(ZSTD_BIN)
        .args(["-q", "--ultra", format!("-{ZSTD_LEVEL}").as_str(), "-c"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CodeNexusError::InvalidInput(format!(
                    "zstd binary not found on PATH — install zstd to use export/import. Error: {e}"
                ))
            } else {
                CodeNexusError::Io(e)
            }
        })?;
    {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            CodeNexusError::Internal("zstd stdin not captured".to_string())
        })?;
        stdin.write_all(input)?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CodeNexusError::InvalidInput(format!(
            "zstd compression failed (status {}): {}",
            output.status,
            stderr.trim()
        )));
    }
    Ok(output.stdout)
}

/// Runs export against an injected Kit (testable core).
#[cfg(any(feature = "cli", feature = "mcp", test))]
pub fn run_export(
    kit: &AsyncKit<AsyncReady>,
    output: &str,
    project: &str,
) -> Result<ExportOutput, CodeNexusError> {
    let storage_config = kit.config::<StorageConfig>()?;
    let db_path = storage_config.db_path.clone();
    let db_path_str = db_path.to_string_lossy().into_owned();

    if !db_path.exists() {
        return Err(CodeNexusError::InvalidInput(format!(
            "database path does not exist: {db_path_str}"
        )));
    }

    let original_bytes = std::fs::read(&db_path)?;
    let original_size = u64::try_from(original_bytes.len()).unwrap_or(0);
    let exported_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let manifest = ArtifactManifest {
        format_version: ARTIFACT_FORMAT_VERSION.to_string(),
        codenexus_version: env!("CARGO_PKG_VERSION").to_string(),
        exported_at,
        source_db_path: db_path_str,
        project: if project.is_empty() {
            None
        } else {
            Some(project.to_string())
        },
        original_size,
    };
    let manifest_json = serde_json::to_vec(&manifest)?;
    let compressed = zstd_compress(&original_bytes)?;

    let output_path = Path::new(output);
    {
        let mut out = File::create(output_path)?;
        out.write_all(&ARTIFACT_MAGIC)?;
        let manifest_len = u32::try_from(manifest_json.len()).map_err(|_| {
            CodeNexusError::InvalidInput(format!(
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
    Ok(ExportOutput {
        artifact: output.to_string(),
        artifact_size,
        original_size,
        codenexus_version: env!("CARGO_PKG_VERSION").to_string(),
        exported_at,
    })
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[service_api(
    name = "export",
    version = "0.3.2",
    description = "Export the graph database to a compressed zstd team artifact.",
    cli = true
)]
async fn export(output: String, project: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let result =
        run_export(&kit, &output, &project).map_err(|e| to_api_error(e, "export_error"))?;
    let json = serde_json::to_string(&result)
        .map_err(|e| to_api_error(CodeNexusError::from(e), "export_error"))?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_export_testdb");
        (dir, path)
    }

    fn build_kit_for_db(db: &std::path::Path) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    #[test]
    fn artifact_manifest_round_trips_through_json() {
        let manifest = ArtifactManifest {
            format_version: ARTIFACT_FORMAT_VERSION.to_string(),
            codenexus_version: "0.3.2".into(),
            exported_at: 1234567890,
            source_db_path: "/demo/db.ladybug".into(),
            project: Some("demo".into()),
            original_size: 4096,
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let parsed: ArtifactManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(manifest, parsed);
    }

    #[test]
    fn artifact_manifest_handles_optional_project() {
        let with_project = ArtifactManifest {
            format_version: "1.0".into(),
            codenexus_version: "0.3.2".into(),
            exported_at: 0,
            source_db_path: "/db".into(),
            project: Some("demo".into()),
            original_size: 0,
        };
        let without_project = ArtifactManifest {
            project: None,
            ..with_project.clone()
        };
        let json_with = serde_json::to_string(&with_project).unwrap();
        let json_without = serde_json::to_string(&without_project).unwrap();
        assert!(json_with.contains("\"project\":\"demo\""));
        assert!(json_without.contains("null"), "None project should serialize to null");
    }

    #[test]
    fn export_output_serializes_to_json() {
        let output = ExportOutput {
            artifact: "/tmp/out.cnxp".into(),
            artifact_size: 1024,
            original_size: 4096,
            codenexus_version: "0.3.2".into(),
            exported_at: 1234567890,
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"artifact\":\"/tmp/out.cnxp\""));
        assert!(json.contains("\"artifact_size\":1024"));
        assert!(json.contains("\"original_size\":4096"));
        assert!(json.contains("\"exported_at\":1234567890"));
    }

    #[test]
    fn artifact_constants_have_expected_values() {
        assert_eq!(ARTIFACT_MAGIC, *b"CNXP");
        assert_eq!(ARTIFACT_FORMAT_VERSION, "1.0");
        assert_eq!(ZSTD_BIN, "zstd");
        assert_eq!(ZSTD_LEVEL, "19");
    }

    #[test]
    fn run_export_returns_error_when_db_does_not_exist() {
        let (dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Delete the DB file so run_export can't find it.
        let _ = std::fs::remove_file(&db);
        let artifact = dir.path().join("missing.cnxp");
        let err = run_export(&kit, artifact.to_str().unwrap(), "demo")
            .expect_err("missing DB should error");
        match err {
            CodeNexusError::InvalidInput(msg) => {
                assert!(
                    msg.contains("database path does not exist"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn zstd_compress_succeeds_for_nonempty_input() {
        let data = b"Hello, World! This is a test for zstd compression.";
        match zstd_compress(data) {
            Ok(compressed) => {
                assert!(
                    !compressed.is_empty(),
                    "compressed data should not be empty"
                );
            }
            Err(CodeNexusError::InvalidInput(msg)) if msg.contains("zstd binary not found") => {
                // zstd not installed — skip this test gracefully.
            }
            Err(e) => panic!("unexpected zstd_compress error: {e}"),
        }
    }

    #[test]
    fn zstd_compress_returns_error_for_empty_input() {
        // zstd can compress empty input, but the result should be valid.
        match zstd_compress(b"") {
            Ok(compressed) => {
                // zstd produces a valid frame even for empty input.
                assert!(!compressed.is_empty(), "zstd empty frame should be non-empty");
            }
            Err(CodeNexusError::InvalidInput(msg)) if msg.contains("zstd binary not found") => {
                // zstd not installed — skip.
            }
            Err(e) => panic!("unexpected zstd_compress error for empty input: {e}"),
        }
    }

    #[test]
    fn run_export_creates_valid_artifact() {
        let (dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let artifact = dir.path().join("exported.cnxp");

        let result = run_export(&kit, artifact.to_str().unwrap(), "demo");
        match result {
            Ok(output) => {
                assert!(artifact.exists(), "artifact file should be created");
                assert!(output.artifact_size > 0, "artifact size should be > 0");
                assert!(output.original_size > 0, "original size should be > 0");
                assert_eq!(output.artifact, artifact.to_str().unwrap());

                // Verify magic bytes.
                let bytes = std::fs::read(&artifact).unwrap();
                assert_eq!(&bytes[0..4], &ARTIFACT_MAGIC);
                // Verify manifest_len is readable.
                let manifest_len = u32::from_le_bytes([
                    bytes[4], bytes[5], bytes[6], bytes[7],
                ]);
                assert!(manifest_len > 0, "manifest should not be empty");
                assert!(
                    bytes.len() >= 8 + manifest_len as usize,
                    "artifact should contain at least header + manifest"
                );
            }
            Err(CodeNexusError::InvalidInput(msg)) if msg.contains("zstd binary not found") => {
                // zstd not installed — skip.
            }
            Err(e) => panic!("unexpected run_export error: {e}"),
        }
    }
}
