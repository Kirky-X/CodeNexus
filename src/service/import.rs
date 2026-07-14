// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Import command: decompress a zstd team artifact into the database.
//!
//! See [`crate::service::export`] for the artifact format and zstd library
//! dependency rationale.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use serde::Serialize;

use crate::service::export::{ArtifactManifest, ARTIFACT_FORMAT_VERSION, ARTIFACT_MAGIC};
use crate::storage::StorageConfig;

#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::kit::{AsyncKit, AsyncReady};
#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::service::error::CodeNexusError;
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::error::{kit_not_initialized, to_api_error};
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::runtime::kit;

#[cfg(feature = "cli")]
use sdforge::forge;
#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;

/// JSON-serializable import-command output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ImportOutput {
    pub artifact: String,
    pub db: String,
    pub db_size: u64,
    pub manifest: ArtifactManifest,
    pub reindexed: bool,
}

/// Decompresses `input` (zstd-compressed bytes) via the `oxiarc-zstd` pure-Rust library.
#[cfg(any(feature = "cli", feature = "mcp", test))]
fn zstd_decompress(input: &[u8]) -> Result<Vec<u8>, CodeNexusError> {
    oxiarc_zstd::decompress(input)
        .map_err(|e| CodeNexusError::Internal(format!("zstd decompression failed: {e}")))
}

/// Runs import against an injected Kit (testable core).
#[cfg(any(feature = "cli", feature = "mcp", test))]
pub fn run_import(
    kit: &AsyncKit<AsyncReady>,
    input: &str,
    reindex: bool,
    path: &str,
    name: &str,
) -> Result<ImportOutput, CodeNexusError> {
    let storage_config = kit.config::<StorageConfig>()?;
    let db_path = storage_config.db_path.clone();
    let db_path_str = db_path.to_string_lossy().into_owned();

    let input_path = Path::new(input);
    if !input_path.exists() {
        return Err(CodeNexusError::InvalidInput(format!(
            "artifact path does not exist: {input}"
        )));
    }

    let artifact_bytes = std::fs::read(input_path)?;
    if artifact_bytes.len() < 8 {
        return Err(CodeNexusError::InvalidInput(format!(
            "artifact too small ({} bytes) — expected at least 8-byte header",
            artifact_bytes.len()
        )));
    }
    if artifact_bytes[0..4] != ARTIFACT_MAGIC {
        return Err(CodeNexusError::InvalidInput(format!(
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
        return Err(CodeNexusError::InvalidInput(format!(
            "artifact truncated — header says manifest is {} bytes but file only has {} bytes total",
            manifest_len,
            artifact_bytes.len()
        )));
    }
    let manifest: ArtifactManifest = serde_json::from_slice(&artifact_bytes[8..header_total])?;
    if manifest.format_version != ARTIFACT_FORMAT_VERSION {
        return Err(CodeNexusError::InvalidInput(format!(
            "artifact format version mismatch — expected {}, got {}",
            ARTIFACT_FORMAT_VERSION, manifest.format_version
        )));
    }

    let compressed_payload = &artifact_bytes[header_total..];
    let db_bytes = zstd_decompress(compressed_payload)?;

    // Remove stale WAL sidecar so LadybugDB doesn't reject the imported DB
    // (the WAL carries the old database ID; importing replaces the DB bytes).
    let wal_path = format!("{}.wal", db_path.display());
    let _ = std::fs::remove_file(&wal_path);

    {
        let mut out = File::create(&db_path)?;
        out.write_all(&db_bytes)?;
        out.flush()?;
    }

    let db_size = db_path.metadata()?.len();

    let mut reindexed = false;
    if reindex {
        if path.is_empty() {
            return Err(CodeNexusError::InvalidInput(
                "--reindex requires --path".to_string(),
            ));
        }
        if name.is_empty() {
            return Err(CodeNexusError::InvalidInput(
                "--reindex requires --name".to_string(),
            ));
        }
        let _output =
            crate::service::index::index_core(kit, &db_path, path, name, false, false, false)?;
        reindexed = true;
    }

    Ok(ImportOutput {
        artifact: input.to_string(),
        db: db_path_str,
        db_size,
        manifest,
        reindexed,
    })
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[forge(
    name = "import",
    version = "0.3.2",
    description = "Import a zstd team artifact into the graph database.",
    cli = true
)]
async fn import(input: String, reindex: bool, path: String, name: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let result = run_import(&kit, &input, reindex, &path, &name)
        .map_err(|e| to_api_error(e, "import_error"))?;
    let json = serde_json::to_string(&result)
        .map_err(|e| to_api_error(CodeNexusError::from(e), "import_error"))?;
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
        let path = dir.path().join("svc_import_testdb");
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
    fn import_output_serializes_to_json() {
        let manifest = ArtifactManifest {
            format_version: ARTIFACT_FORMAT_VERSION.to_string(),
            codenexus_version: "0.3.2".into(),
            exported_at: 1000,
            source_db_path: "/db".into(),
            project: Some("demo".into()),
            original_size: 4096,
        };
        let output = ImportOutput {
            artifact: "/tmp/a.cnxp".into(),
            db: "/db/path".into(),
            db_size: 2048,
            manifest,
            reindexed: false,
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"artifact\":\"/tmp/a.cnxp\""));
        assert!(json.contains("\"db\":\"/db/path\""));
        assert!(json.contains("\"db_size\":2048"));
        assert!(json.contains("\"reindexed\":false"));
        assert!(json.contains("\"format_version\":\"1.0\""));
    }

    #[test]
    fn zstd_decompress_returns_error_for_invalid_input() {
        let garbage = b"this is not valid zstd data at all";
        let err = zstd_decompress(garbage).expect_err("decompressing garbage should fail");
        // zstd library returns io::Error for invalid data, converted to CodeNexusError::Io.
        let msg = err.to_string();
        assert!(!msg.is_empty(), "error should have a message");
    }

    #[test]
    fn zstd_decompress_succeeds_for_valid_compressed_data() {
        let original = b"Hello, zstd! This is a test for round-trip decompression.";
        let compressed = oxiarc_zstd::compress_with_level(&original[..], 19).expect("zstd encode");
        let decompressed = zstd_decompress(&compressed).expect("zstd_decompress should succeed");
        assert_eq!(decompressed, original, "decompressed should match original");
    }

    #[test]
    fn zstd_decompress_returns_error_for_empty_input() {
        let err = zstd_decompress(b"").expect_err("empty input should fail decompression");
        let msg = err.to_string();
        assert!(!msg.is_empty(), "error should have a message");
    }

    #[test]
    fn run_import_returns_error_for_nonexistent_artifact() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let err = run_import(&kit, "/nonexistent/path/artifact.cnxp", false, "", "")
            .expect_err("nonexistent artifact should error");
        match err {
            CodeNexusError::InvalidInput(msg) => {
                assert!(
                    msg.contains("artifact path does not exist"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn run_import_returns_error_for_too_small_artifact() {
        let (dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let artifact = dir.path().join("small.cnxp");
        std::fs::write(&artifact, b"CNXP").unwrap(); // Only 4 bytes, < 8
        let err = run_import(&kit, artifact.to_str().unwrap(), false, "", "")
            .expect_err("too-small artifact should error");
        match err {
            CodeNexusError::InvalidInput(msg) => {
                assert!(
                    msg.contains("artifact too small"),
                    "unexpected message: {msg}"
                );
                assert!(msg.contains("4 bytes"), "should mention size: {msg}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn run_import_returns_error_for_bad_magic() {
        let (dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let artifact = dir.path().join("badmagic.cnxp");
        let mut data = b"BADM".to_vec();
        data.extend(&[0u8, 0, 0, 0]); // manifest_len = 0
        std::fs::write(&artifact, &data).unwrap();
        let err = run_import(&kit, artifact.to_str().unwrap(), false, "", "")
            .expect_err("bad magic should error");
        match err {
            CodeNexusError::InvalidInput(msg) => {
                assert!(msg.contains("magic mismatch"), "unexpected message: {msg}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn run_import_returns_error_for_truncated_artifact() {
        let (dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let artifact = dir.path().join("truncated.cnxp");
        let mut data = ARTIFACT_MAGIC.to_vec();
        data.extend(&100u32.to_le_bytes()); // manifest_len = 100
        data.extend(b"{ }"); // Only 2 bytes, but header says 100
        std::fs::write(&artifact, &data).unwrap();
        let err = run_import(&kit, artifact.to_str().unwrap(), false, "", "")
            .expect_err("truncated artifact should error");
        match err {
            CodeNexusError::InvalidInput(msg) => {
                assert!(
                    msg.contains("artifact truncated"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn run_import_returns_error_for_version_mismatch() {
        let (dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let artifact = dir.path().join("badversion.cnxp");
        let manifest = ArtifactManifest {
            format_version: "0.0".to_string(), // Wrong version
            codenexus_version: "0.3.2".into(),
            exported_at: 0,
            source_db_path: "/dummy".into(),
            project: None,
            original_size: 0,
        };
        let manifest_json = serde_json::to_vec(&manifest).unwrap();
        let mut data = ARTIFACT_MAGIC.to_vec();
        data.extend(&(manifest_json.len() as u32).to_le_bytes());
        data.extend(&manifest_json);
        data.extend(b"fake_compressed_data");
        std::fs::write(&artifact, &data).unwrap();
        let err = run_import(&kit, artifact.to_str().unwrap(), false, "", "")
            .expect_err("version mismatch should error");
        match err {
            CodeNexusError::InvalidInput(msg) => {
                assert!(
                    msg.contains("version mismatch"),
                    "unexpected message: {msg}"
                );
                assert!(msg.contains("0.0"), "should mention bad version: {msg}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    // Covers line 127: serde_json::from_slice error path when manifest JSON
    // is malformed (valid magic + manifest_len, but invalid JSON content).
    #[test]
    fn run_import_returns_error_for_malformed_manifest_json() {
        let (dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let artifact = dir.path().join("malformed.cnxp");
        let bad_manifest = b"{ this is not valid json";
        let mut data = ARTIFACT_MAGIC.to_vec();
        data.extend(&(bad_manifest.len() as u32).to_le_bytes());
        data.extend(bad_manifest);
        data.extend(b"fake_compressed_data");
        std::fs::write(&artifact, &data).unwrap();
        let err = run_import(&kit, artifact.to_str().unwrap(), false, "", "")
            .expect_err("malformed manifest JSON should error");
        // serde_json::from_slice error is wrapped via `?` on line 127,
        // producing a serde_json error (converted to CodeNexusError).
        // The error should NOT be InvalidInput (that's for our explicit checks);
        // it should be a serialization/deserialization error.
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("json")
                || msg.to_lowercase().contains("deserialize")
                || msg.to_lowercase().contains("parse"),
            "unexpected error for malformed manifest: {msg}"
        );
    }

    // Covers the case where the manifest_len is 0 (header says 0 bytes for
    // manifest). This triggers serde_json::from_slice on an empty slice,
    // which returns an EOF error.
    #[test]
    fn run_import_returns_error_for_zero_manifest_len() {
        let (dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let artifact = dir.path().join("zerolen.cnxp");
        let mut data = ARTIFACT_MAGIC.to_vec();
        data.extend(&0u32.to_le_bytes()); // manifest_len = 0
        data.extend(b"fake_compressed_data");
        std::fs::write(&artifact, &data).unwrap();
        let err = run_import(&kit, artifact.to_str().unwrap(), false, "", "")
            .expect_err("zero manifest_len should error");
        let msg = err.to_string();
        // Empty slice → serde_json EOF error
        assert!(!msg.is_empty(), "error message should not be empty");
    }

    #[test]
    fn run_import_returns_error_for_reindex_without_path() {
        let (dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let artifact = dir.path().join("valid.cnxp");
        let manifest = ArtifactManifest {
            format_version: ARTIFACT_FORMAT_VERSION.to_string(),
            codenexus_version: "0.3.2".into(),
            exported_at: 0,
            source_db_path: "/dummy".into(),
            project: None,
            original_size: 0,
        };
        let manifest_json = serde_json::to_vec(&manifest).unwrap();
        let compressed =
            oxiarc_zstd::compress_with_level(&b"fake_db_bytes"[..], 19).expect("zstd encode");
        let mut data = ARTIFACT_MAGIC.to_vec();
        data.extend(&(manifest_json.len() as u32).to_le_bytes());
        data.extend(&manifest_json);
        data.extend(&compressed);
        std::fs::write(&artifact, &data).unwrap();

        let err = run_import(&kit, artifact.to_str().unwrap(), true, "", "demo")
            .expect_err("reindex without path should error");
        match err {
            CodeNexusError::InvalidInput(msg) => {
                assert!(
                    msg.contains("--reindex requires --path"),
                    "should mention --reindex requires --path: {msg}"
                );
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn run_import_returns_error_for_reindex_without_name() {
        let (dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let artifact = dir.path().join("valid2.cnxp");
        let manifest = ArtifactManifest {
            format_version: ARTIFACT_FORMAT_VERSION.to_string(),
            codenexus_version: "0.3.2".into(),
            exported_at: 0,
            source_db_path: "/dummy".into(),
            project: None,
            original_size: 0,
        };
        let manifest_json = serde_json::to_vec(&manifest).unwrap();
        let compressed =
            oxiarc_zstd::compress_with_level(&b"fake_db_bytes"[..], 19).expect("zstd encode");
        let mut data = ARTIFACT_MAGIC.to_vec();
        data.extend(&(manifest_json.len() as u32).to_le_bytes());
        data.extend(&manifest_json);
        data.extend(&compressed);
        std::fs::write(&artifact, &data).unwrap();

        let err = run_import(&kit, artifact.to_str().unwrap(), true, "/some/path", "")
            .expect_err("reindex without name should error");
        match err {
            CodeNexusError::InvalidInput(msg) => {
                assert!(
                    msg.contains("--reindex requires --name"),
                    "should mention --reindex requires --name: {msg}"
                );
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    // Round-trip test: export a DB to an artifact, then import it back.
    // Covers zstd_decompress success path (lines 62-76) and run_import
    // file-write success path (lines 139-168).
    #[test]
    fn run_import_round_trips_with_export_artifact() {
        let (src_dir, src_db) = fresh_db_path();
        let src_kit = build_kit_for_db(&src_db);
        let artifact = src_dir.path().join("roundtrip.cnxp");

        let export_output =
            crate::service::export::run_export(&src_kit, artifact.to_str().unwrap(), "demo")
                .expect("run_export should succeed");

        let (_dst_dir, dst_db) = fresh_db_path();
        let dst_kit = build_kit_for_db(&dst_db);
        let import_output = run_import(&dst_kit, artifact.to_str().unwrap(), false, "", "")
            .expect("import should succeed for valid artifact");

        assert_eq!(import_output.artifact, artifact.to_str().unwrap());
        assert!(import_output.db_size > 0, "db_size should be > 0");
        assert!(!import_output.reindexed);
        assert_eq!(
            import_output.manifest.format_version,
            ARTIFACT_FORMAT_VERSION
        );
        assert_eq!(import_output.manifest.project.as_deref(), Some("demo"));
        assert_eq!(
            import_output.manifest.original_size,
            export_output.original_size
        );
    }

    // Covers run_import reindex success path (lines 146-160).
    #[cfg(feature = "lang-rust")]
    #[test]
    fn run_import_with_reindex_succeeds() {
        let (src_dir, src_db) = fresh_db_path();
        let src_kit = build_kit_for_db(&src_db);
        let artifact = src_dir.path().join("reindex.cnxp");

        crate::service::export::run_export(&src_kit, artifact.to_str().unwrap(), "demo")
            .expect("run_export should succeed");

        let reindex_src = TempDir::new().unwrap();
        std::fs::write(
            reindex_src.path().join("lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        )
        .unwrap();

        let (_dst_dir, dst_db) = fresh_db_path();
        let dst_kit = build_kit_for_db(&dst_db);
        let import_output = run_import(
            &dst_kit,
            artifact.to_str().unwrap(),
            true,
            reindex_src.path().to_str().unwrap(),
            "reindexed_project",
        )
        .expect("import with reindex should succeed");

        assert!(
            import_output.reindexed,
            "reindex=true → reindexed should be true"
        );
        assert!(import_output.db_size > 0);
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

    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn import_wrapper_fails_with_nonexistent_artifact() {
        let _guard = KitGuard::new();
        let (_dir, db) = fresh_db_path();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let kit = rt.block_on(async {
            let config = KitBootstrapConfig::new(db.to_path_buf());
            build_kit(&config).await.expect("build_kit")
        });
        crate::service::runtime::force_init_kit_for_testing(kit);
        let result = rt.block_on(import(
            "/nonexistent/path/artifact.cnxp".to_string(),
            false,
            String::new(),
            String::new(),
        ));
        let err = result.expect_err("nonexistent artifact should error");
        assert!(
            matches!(err, ApiError::InvalidInput { .. }),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn import_wrapper_fails_when_kit_not_initialized() {
        let _guard = KitGuard::new();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(import(
            "/nonexistent/path/artifact.cnxp".to_string(),
            false,
            String::new(),
            String::new(),
        ));
        assert!(result.is_err(), "wrapper should fail without kit");
    }

    // Covers the import wrapper success path (lines 180-188):
    // kit() resolves, run_export creates artifact, run_import succeeds,
    // serde_json::to_string succeeds, println outputs JSON.
    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn import_wrapper_succeeds_via_init_kit() {
        let _guard = KitGuard::new();

        let (src_dir, src_db) = fresh_db_path();
        let src_rt = tokio::runtime::Runtime::new().expect("runtime");
        let src_kit = src_rt.block_on(async {
            let config = KitBootstrapConfig::new(src_db.to_path_buf());
            build_kit(&config).await.expect("build_kit")
        });
        let artifact = src_dir.path().join("wrapper_roundtrip.cnxp");

        // Export to create a valid artifact.
        crate::service::export::run_export(&src_kit, artifact.to_str().unwrap(), "demo")
            .expect("run_export should succeed");

        // Set up a fresh kit for the import side.
        let (_dst_dir, dst_db) = fresh_db_path();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let dst_kit = rt.block_on(async {
            let config = KitBootstrapConfig::new(dst_db.to_path_buf());
            build_kit(&config).await.expect("build_kit")
        });
        crate::service::runtime::force_init_kit_for_testing(dst_kit);
        let result = rt.block_on(import(
            artifact.to_string_lossy().into_owned(),
            false,
            String::new(),
            String::new(),
        ));
        assert!(
            result.is_ok(),
            "import wrapper should succeed: {:?}",
            result.err()
        );
    }

    // Covers the import wrapper with reindex=false and a nonexistent artifact
    // through the #[forge] wrapper → ApiError conversion path.
    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn import_wrapper_with_malformed_artifact_returns_api_error() {
        let _guard = KitGuard::new();
        let (dir, db) = fresh_db_path();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let kit = rt.block_on(async {
            let config = KitBootstrapConfig::new(db.to_path_buf());
            build_kit(&config).await.expect("build_kit")
        });
        crate::service::runtime::force_init_kit_for_testing(kit);

        let bad_artifact = dir.path().join("too_small.cnxp");
        std::fs::write(&bad_artifact, b"CNXP").unwrap();

        let result = rt.block_on(import(
            bad_artifact.to_string_lossy().into_owned(),
            false,
            String::new(),
            String::new(),
        ));
        let err = result.expect_err("too-small artifact should error");
        assert!(
            matches!(err, ApiError::InvalidInput { .. }),
            "expected InvalidInput, got {err:?}"
        );
    }
}
