// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Clean command: remove a project by name or id.

use serde::Serialize;
use serde_json::Value;

use crate::kit::StorageKey;
use crate::service::error::{kit_not_initialized, wrap_error};
use crate::service::runtime::kit;

#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::service_api;

/// JSON-serializable clean-command output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CleanOutput {
    pub project: String,
    pub project_id: String,
    pub deleted: usize,
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[service_api(
    name = "clean",
    version = "0.3.2",
    description = "Remove a project and its index by name or id.",
    cli = true
)]
async fn clean(project: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let storage = kit
        .require::<StorageKey>()
        .map_err(|e| wrap_error("Failed to resolve storage capability", e))?;

    // Find the project by name first, then by id.
    let projects = storage
        .list_projects()
        .map_err(|e| wrap_error("Failed to list projects", e))?;
    let project_id = projects
        .iter()
        .find(|p| p.name == project)
        .map(|p| p.id.clone())
        .or_else(|| {
            if projects.iter().any(|p| p.id == project) {
                Some(project.clone())
            } else {
                None
            }
        });

    let project_id = match project_id {
        Some(id) => id,
        None => {
            return Err(ApiError::InvalidInput {
                message: format!("project not found: {project}"),
                field: Some("project".to_string()),
                value: Some(Value::String(project)),
            });
        }
    };

    storage
        .delete_project(&project_id)
        .map_err(|e| wrap_error("Failed to delete project", e))?;
    let output = CleanOutput {
        project,
        project_id,
        deleted: 1,
    };
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}
