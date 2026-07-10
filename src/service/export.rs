// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Export command: compress the database to a zstd team artifact.
//!
//! See `crate::cli::export_cmd` for the artifact format and zstd CLI
//! dependency rationale.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::kit::StorageConfigKey;
use crate::service::error::{kit_not_initialized, wrap_error};
use crate::service::runtime::kit;

#[cfg(feature = "cli")]
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
fn zstd_compress(input: &[u8]) -> Result<Vec<u8>, ApiError> {
    let mut child = Command::new(ZSTD_BIN)
        .args(["-q", "--ultra", format!("-{ZSTD_LEVEL}").as_str(), "-c"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ApiError::InvalidInput {
                    message: format!(
                        "zstd binary not found on PATH — install zstd to use export/import. Error: {e}"
                    ),
                    field: None,
                    value: None,
                }
            } else {
                wrap_error("Failed to spawn zstd", e)
            }
        })?;
    {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            ApiError::internal_error("zstd stdin not captured", "zstd_stdin_capture")
        })?;
        stdin
            .write_all(input)
            .map_err(|e| wrap_error("zstd stdin write failed", e))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| wrap_error("zstd wait failed", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ApiError::InvalidInput {
            message: format!(
                "zstd compression failed (status {}): {}",
                output.status,
                stderr.trim()
            ),
            field: None,
            value: None,
        });
    }
    Ok(output.stdout)
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[service_api(
    name = "codenexus",
    version = "0.3.2",
    tool_name = "export",
    description = "Export the graph database to a compressed zstd team artifact.",
    cli = true,
)]
async fn export(output: String, project: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let storage_config = kit
        .config::<StorageConfigKey>()
        .map_err(|e| wrap_error("Failed to resolve storage config", e))?;
    let storage_config = storage_config.load();
    let db_path = storage_config.db_path.clone();

    let db_path_str = db_path.to_string_lossy().into_owned();
    if !db_path.exists() {
        return Err(ApiError::InvalidInput {
            message: format!("database path does not exist: {db_path_str}"),
            field: Some("db".to_string()),
            value: Some(serde_json::Value::String(db_path_str)),
        });
    }

    let original_bytes = std::fs::read(&db_path)
        .map_err(|e| wrap_error("Failed to read database file", e))?;
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
        project: if project.is_empty() { None } else { Some(project) },
        original_size,
    };
    let manifest_json = serde_json::to_vec(&manifest)
        .map_err(|e| wrap_error("Manifest serialization failed", e))?;

    let compressed = zstd_compress(&original_bytes)?;

    // Build the artifact: magic | manifest_len (u32 LE) | manifest | zstd(db).
    let output_path = Path::new(&output);
    {
        let mut out = File::create(output_path)
            .map_err(|e| wrap_error("Failed to create artifact file", e))?;
        out.write_all(&ARTIFACT_MAGIC)
            .map_err(|e| wrap_error("Failed to write magic", e))?;
        let manifest_len = u32::try_from(manifest_json.len()).map_err(|_| {
            ApiError::InvalidInput {
                message: format!(
                    "manifest too large ({} bytes) — exceeds u32::MAX",
                    manifest_json.len()
                ),
                field: None,
                value: None,
            }
        })?;
        out.write_all(&manifest_len.to_le_bytes())
            .map_err(|e| wrap_error("Failed to write manifest length", e))?;
        out.write_all(&manifest_json)
            .map_err(|e| wrap_error("Failed to write manifest", e))?;
        out.write_all(&compressed)
            .map_err(|e| wrap_error("Failed to write compressed data", e))?;
        out.flush()
            .map_err(|e| wrap_error("Failed to flush artifact file", e))?;
    }

    let artifact_size = output_path
        .metadata()
        .map_err(|e| wrap_error("Failed to read artifact metadata", e))?
        .len();
    let output = ExportOutput {
        artifact: output,
        artifact_size,
        original_size,
        codenexus_version: env!("CARGO_PKG_VERSION").to_string(),
        exported_at,
    };
    let json = serde_json::to_string(&output)
        .map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}
