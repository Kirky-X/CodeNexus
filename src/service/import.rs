// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Import command: decompress a zstd team artifact into the database.
//!
//! See `crate::cli::import_cmd` for the artifact format and zstd CLI
//! dependency rationale.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::Serialize;
use serde_json::Value;

use crate::cli::args::IndexArgs;
use crate::kit::StorageConfigKey;
use crate::service::error::{kit_not_initialized, wrap_error};
use crate::service::export::{ArtifactManifest, ARTIFACT_FORMAT_VERSION, ARTIFACT_MAGIC, ZSTD_BIN};
use crate::service::runtime::kit;

#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::service_api;

/// JSON-serializable import-command output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ImportOutput {
    pub artifact: String,
    pub db: String,
    pub db_size: u64,
    pub manifest: ArtifactManifest,
    pub reindexed: bool,
}

/// Decompresses `input` (zstd-compressed bytes) via the system `zstd` CLI.
fn zstd_decompress(input: &[u8]) -> Result<Vec<u8>, ApiError> {
    let mut child = Command::new(ZSTD_BIN)
        .args(["-q", "-d", "-c"])
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
                "zstd decompression failed (status {}): {}",
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
    tool_name = "import",
    description = "Import a zstd team artifact into the graph database.",
    cli = true,
)]
async fn import(
    input: String,
    reindex: bool,
    path: String,
    name: String,
) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let storage_config = kit
        .config::<StorageConfigKey>()
        .map_err(|e| wrap_error("Failed to resolve storage config", e))?;
    let storage_config = storage_config.load();
    let db_path = storage_config.db_path.clone();
    let db_path_str = db_path.to_string_lossy().into_owned();

    let input_path = Path::new(&input);
    if !input_path.exists() {
        return Err(ApiError::InvalidInput {
            message: format!("artifact path does not exist: {input}"),
            field: Some("input".to_string()),
            value: Some(Value::String(input)),
        });
    }

    let artifact_bytes = std::fs::read(input_path)
        .map_err(|e| wrap_error("Failed to read artifact file", e))?;
    if artifact_bytes.len() < 8 {
        return Err(ApiError::InvalidInput {
            message: format!(
                "artifact too small ({} bytes) — expected at least 8-byte header",
                artifact_bytes.len()
            ),
            field: None,
            value: None,
        });
    }
    if artifact_bytes[0..4] != ARTIFACT_MAGIC {
        return Err(ApiError::InvalidInput {
            message: format!(
                "artifact magic mismatch — expected {:?}, got {:?}",
                std::str::from_utf8(&ARTIFACT_MAGIC),
                std::str::from_utf8(&artifact_bytes[0..4])
            ),
            field: None,
            value: None,
        });
    }
    let manifest_len = u32::from_le_bytes([
        artifact_bytes[4],
        artifact_bytes[5],
        artifact_bytes[6],
        artifact_bytes[7],
    ]) as usize;
    let header_total = 8 + manifest_len;
    if artifact_bytes.len() < header_total {
        return Err(ApiError::InvalidInput {
            message: format!(
                "artifact truncated — header says manifest is {} bytes but file only has {} bytes total",
                manifest_len,
                artifact_bytes.len()
            ),
            field: None,
            value: None,
        });
    }
    let manifest: ArtifactManifest = serde_json::from_slice(&artifact_bytes[8..header_total])
        .map_err(|e| wrap_error("Manifest deserialization failed", e))?;
    if manifest.format_version != ARTIFACT_FORMAT_VERSION {
        return Err(ApiError::InvalidInput {
            message: format!(
                "artifact format version mismatch — expected {}, got {}",
                ARTIFACT_FORMAT_VERSION, manifest.format_version
            ),
            field: None,
            value: None,
        });
    }

    let compressed_payload = &artifact_bytes[header_total..];
    let db_bytes = zstd_decompress(compressed_payload)?;

    {
        let mut out = File::create(&db_path)
            .map_err(|e| wrap_error("Failed to create database file", e))?;
        out.write_all(&db_bytes)
            .map_err(|e| wrap_error("Failed to write database bytes", e))?;
        out.flush()
            .map_err(|e| wrap_error("Failed to flush database file", e))?;
    }

    let db_size = db_path
        .metadata()
        .map_err(|e| wrap_error("Failed to read database metadata", e))?
        .len();

    // Optionally trigger an incremental reindex.
    let mut reindexed = false;
    if reindex {
        if path.is_empty() {
            return Err(ApiError::InvalidInput {
                message: "--reindex requires --path".to_string(),
                field: Some("path".to_string()),
                value: None,
            });
        }
        if name.is_empty() {
            return Err(ApiError::InvalidInput {
                message: "--reindex requires --name".to_string(),
                field: Some("name".to_string()),
                value: None,
            });
        }
        let index_args = IndexArgs {
            path: path.clone(),
            name: name.clone(),
            db: db_path_str.clone(),
            force: false,
            lsp: false,
            embed: false,
            ram_first: false,
        };
        crate::cli::index_cmd::run(kit, &index_args)
            .map_err(|e| wrap_error("Reindex failed", e))?;
        reindexed = true;
    }

    let output = ImportOutput {
        artifact: input,
        db: db_path_str,
        db_size,
        manifest,
        reindexed,
    };
    let json = serde_json::to_string(&output)
        .map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}
