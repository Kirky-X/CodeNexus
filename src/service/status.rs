// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Status command: list indexed projects with staleness check.

use std::path::Path;

use serde::Serialize;

use crate::kit::StorageKey;
use crate::service::error::{kit_not_initialized, wrap_error};
use crate::service::runtime::kit;
use crate::storage::ProjectRecord;

#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::service_api;

/// JSON-serializable status output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StatusOutput {
    pub projects: Vec<ProjectOutput>,
}

/// JSON-serializable view of a project with staleness info.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ProjectOutput {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub language: String,
    pub file_count: i64,
    pub indexed_at: i64,
    pub last_commit: String,
    pub current_head: String,
    pub stale: bool,
}

impl ProjectOutput {
    fn from_record(rec: ProjectRecord, current_head: String, stale: bool) -> Self {
        Self {
            id: rec.id,
            name: rec.name,
            root_path: rec.root_path,
            language: rec.language,
            file_count: rec.file_count,
            indexed_at: rec.indexed_at,
            last_commit: rec.last_commit,
            current_head,
            stale,
        }
    }
}

/// Returns the current `HEAD` commit hash of the git repo at `root`, or an
/// empty string if `root` is not a git repo (or git is unavailable).
fn git_head_commit(root: &Path) -> String {
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[service_api(
    name = "status",
    version = "0.3.2",
    description = "List all indexed projects and check their staleness.",
    cli = true
)]
async fn status() -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let storage = kit
        .require::<StorageKey>()
        .map_err(|e| wrap_error("Failed to resolve storage capability", e))?;
    let projects = storage
        .list_projects()
        .map_err(|e| wrap_error("Failed to list projects", e))?;
    let output = StatusOutput {
        projects: projects
            .into_iter()
            .map(|p| {
                let current_head = git_head_commit(Path::new(&p.root_path));
                let stale = !p.last_commit.is_empty()
                    && !current_head.is_empty()
                    && p.last_commit != current_head;
                ProjectOutput::from_record(p, current_head, stale)
            })
            .collect(),
    };
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}
