// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Export command: compress the database to a zstd team artifact.
//!
//! Artifact format: `[magic (4B)] [manifest_len (4B LE)] [manifest JSON] [zstd-compressed DB bytes]`.
//! Compression is handled by the `zstd` library (no external CLI binary required).

use std::fs::File;
use std::io::Write;
use std::path::Path;
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
use sdforge::forge;

/// Artifact magic bytes — "CNXP" (CodeNexus eXPort).
pub const ARTIFACT_MAGIC: [u8; 4] = *b"CNXP";

/// Current artifact format version.
pub const ARTIFACT_FORMAT_VERSION: &str = "1.0";

/// Name of the zstd CLI binary formerly invoked for compression.
/// Retained for backward compatibility; compression now uses the `zstd` library.
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

/// Compresses `input` bytes via the `oxiarc-zstd` pure-Rust library.
#[cfg(any(feature = "cli", feature = "mcp", test))]
fn zstd_compress(input: &[u8]) -> Result<Vec<u8>, CodeNexusError> {
    let level: i32 = ZSTD_LEVEL.parse().unwrap_or(19);
    oxiarc_zstd::compress_with_level(input, level)
        .map_err(|e| CodeNexusError::Internal(format!("zstd compression failed: {e}")))
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
#[forge(
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
        let compressed = zstd_compress(data).expect("zstd_compress should succeed");
        assert!(!compressed.is_empty(), "compressed data should not be empty");
        assert_ne!(&compressed[..], &data[..], "compressed should differ from original");
    }

    #[test]
    fn zstd_compress_succeeds_for_empty_input() {
        let compressed = zstd_compress(b"").expect("zstd_compress should succeed for empty input");
        assert!(!compressed.is_empty(), "zstd empty frame should be non-empty");
    }

    #[test]
    fn zstd_compress_round_trips_with_decompress() {
        let data = b"Round-trip test: compress then decompress should yield original.";
        let compressed = zstd_compress(data).expect("compress");
        let decompressed = oxiarc_zstd::decompress(&compressed[..]).expect("decompress");
        assert_eq!(decompressed, data, "round-trip should preserve data");
    }

    #[test]
    fn run_export_creates_valid_artifact() {
        let (dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let artifact = dir.path().join("exported.cnxp");

        let output = run_export(&kit, artifact.to_str().unwrap(), "demo")
            .expect("run_export should succeed");
        assert!(artifact.exists(), "artifact file should be created");
        assert!(output.artifact_size > 0, "artifact size should be > 0");
        assert!(output.original_size > 0, "original size should be > 0");
        assert_eq!(output.artifact, artifact.to_str().unwrap());

        // Verify magic bytes.
        let bytes = std::fs::read(&artifact).unwrap();
        assert_eq!(&bytes[0..4], &ARTIFACT_MAGIC);
        let manifest_len = u32::from_le_bytes([
            bytes[4], bytes[5], bytes[6], bytes[7],
        ]);
        assert!(manifest_len > 0, "manifest should not be empty");
        assert!(
            bytes.len() >= 8 + manifest_len as usize,
            "artifact should contain at least header + manifest"
        );
    }

    // Covers run_export with empty project name → manifest.project = None
    // (lines 131-135 None branch).
    #[test]
    fn run_export_with_empty_project_uses_none_in_manifest() {
        let (dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let artifact = dir.path().join("empty_project.cnxp");

        let output = run_export(&kit, artifact.to_str().unwrap(), "")
            .expect("run_export should succeed");
        assert!(artifact.exists());
        let bytes = std::fs::read(&artifact).unwrap();
        let manifest_len = u32::from_le_bytes([
            bytes[4], bytes[5], bytes[6], bytes[7],
        ]) as usize;
        let manifest: ArtifactManifest =
            serde_json::from_slice(&bytes[8..8 + manifest_len]).unwrap();
        assert!(
            manifest.project.is_none(),
            "empty project → None in manifest"
        );
        assert_eq!(output.artifact, artifact.to_str().unwrap());
    }

    // Covers line 143: File::create error path when the output directory
    // does not exist.
    #[test]
    fn run_export_returns_error_when_output_directory_does_not_exist() {
        let (dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Point output to a path under a nonexistent directory.
        let artifact = dir.path().join("nonexistent_dir").join("out.cnxp");
        match run_export(&kit, artifact.to_str().unwrap(), "demo") {
            Err(e) => {
                // Should be an IO error (File::create fails because parent dir
                // doesn't exist).
                let msg = e.to_string();
                assert!(
                    !msg.is_empty(),
                    "error should have a message"
                );
            }
            Ok(_) => panic!("should fail when output directory doesn't exist"),
        }
    }

    // Covers manifest content verification: codenexus_version, exported_at,
    // source_db_path, original_size fields are populated correctly.
    #[test]
    fn run_export_manifest_contains_all_expected_fields() {
        let (dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let artifact = dir.path().join("full_manifest.cnxp");

        let output = run_export(&kit, artifact.to_str().unwrap(), "test_project")
            .expect("run_export should succeed");
        let bytes = std::fs::read(&artifact).unwrap();
        let manifest_len = u32::from_le_bytes([
            bytes[4], bytes[5], bytes[6], bytes[7],
        ]) as usize;
        let manifest: ArtifactManifest =
            serde_json::from_slice(&bytes[8..8 + manifest_len]).unwrap();
        assert_eq!(manifest.format_version, ARTIFACT_FORMAT_VERSION);
        assert!(!manifest.codenexus_version.is_empty());
        assert!(manifest.exported_at > 0, "exported_at should be set");
        assert!(
            manifest.source_db_path.contains("svc_export_testdb"),
            "source_db_path should contain db filename"
        );
        assert_eq!(manifest.project.as_deref(), Some("test_project"));
        assert!(manifest.original_size > 0, "original_size should be > 0");
        assert_eq!(output.codenexus_version, manifest.codenexus_version);
        assert_eq!(output.exported_at, manifest.exported_at);
        assert_eq!(output.original_size, manifest.original_size);
    }

    // ===== #[forge] wrapper tests via init_kit =====

    /// RAII guard that resets the global Kit on Drop, ensuring test isolation.
    #[cfg(feature = "cli")]
    struct KitGuard;

    #[cfg(feature = "cli")]
    impl KitGuard {
        fn new() -> Self {
            crate::service::runtime::force_reset_kit_for_testing();
            Self
        }
    }

    #[cfg(feature = "cli")]
    impl Drop for KitGuard {
        fn drop(&mut self) {
            crate::service::runtime::force_reset_kit_for_testing();
        }
    }

    #[serial_test::serial]
    #[cfg(feature = "cli")]
    #[test]
    fn export_wrapper_succeeds_via_init_kit() {
        let _guard = KitGuard::new();
        let (dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        crate::service::runtime::force_init_kit_for_testing(kit);

        let artifact = dir.path().join("wrapper_export.cnxp");
        rt.block_on(export(
            artifact.to_string_lossy().into_owned(),
            "demo".to_string(),
        ))
        .expect("export wrapper should succeed");
        assert!(artifact.exists(), "artifact file should be created");
    }

    #[cfg(feature = "cli")]
    #[test]
    fn export_wrapper_fails_when_kit_not_initialized() {
        let _guard = KitGuard::new();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(export("/tmp/out.cnxp".to_string(), "demo".to_string()));
        assert!(result.is_err(), "wrapper should fail without kit");
    }

    // Covers the export wrapper failing when DB doesn't exist (line 113-117
    // error path through the #[forge] wrapper → ApiError conversion).
    #[serial_test::serial]
    #[cfg(feature = "cli")]
    #[test]
    fn export_wrapper_fails_when_db_does_not_exist() {
        let _guard = KitGuard::new();
        let (dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let _ = std::fs::remove_file(&db);
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        crate::service::runtime::force_init_kit_for_testing(kit);

        let artifact = dir.path().join("missing_db.cnxp");
        let result = rt.block_on(export(
            artifact.to_string_lossy().into_owned(),
            "demo".to_string(),
        ));
        let err = result.expect_err("missing DB should error");
        assert!(
            matches!(err, ApiError::InvalidInput { .. }),
            "expected InvalidInput for missing DB, got {err:?}"
        );
    }

    // Covers the export wrapper with empty project string (None branch in
    // manifest construction through the wrapper).
    #[serial_test::serial]
    #[cfg(feature = "cli")]
    #[test]
    fn export_wrapper_succeeds_with_empty_project() {
        let _guard = KitGuard::new();
        let (dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        crate::service::runtime::force_init_kit_for_testing(kit);

        let artifact = dir.path().join("empty_proj_wrapper.cnxp");
        rt.block_on(export(
            artifact.to_string_lossy().into_owned(),
            String::new(),
        ))
        .expect("export wrapper should succeed with empty project");
        assert!(artifact.exists(), "artifact should be created");
    }
}
