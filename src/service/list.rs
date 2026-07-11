// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! List command: list all indexed projects.

use serde::Serialize;

use crate::kit::StorageModule;
use crate::service::error::{kit_not_initialized, wrap_error, wrap_kit_error};
use crate::service::runtime::kit;
use crate::storage::ProjectRecord;

#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::service_api;

/// JSON-serializable view of a project record.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ProjectOutput {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub language: String,
    pub file_count: i64,
    pub indexed_at: i64,
    pub last_commit: String,
}

impl From<ProjectRecord> for ProjectOutput {
    fn from(p: ProjectRecord) -> Self {
        Self {
            id: p.id,
            name: p.name,
            root_path: p.root_path,
            language: p.language,
            file_count: p.file_count,
            indexed_at: p.indexed_at,
            last_commit: p.last_commit,
        }
    }
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[service_api(
    name = "list",
    version = "0.3.2",
    description = "List all indexed projects in the database.",
    cli = true
)]
async fn list() -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let storage = kit
        .require::<StorageModule>()
        .map_err(|e| wrap_kit_error("Failed to resolve storage capability", e))?;
    let projects = storage
        .list_projects()
        .map_err(|e| wrap_error("Failed to list projects", e))?;
    let output: Vec<ProjectOutput> = projects.into_iter().map(ProjectOutput::from).collect();
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}
