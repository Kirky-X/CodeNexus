// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Git-verified source evidence ("SRC n" badges).
//!
//! Ports archify's repository-evidence contract: a component may declare up
//! to three source files, and this module verifies each declaration against
//! a pinned revision through a git command chain — toplevel check, revision
//! existence, blob type, and line-range coverage. Paths that could escape
//! the repository (`..`, `.git`, backslashes, control characters) are
//! rejected before any git invocation. Verification never talks to the
//! network; blob links are derived from the declared origin URL only.

use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

use super::ir::SourceRef;

/// Errors from the evidence verification chain.
#[derive(Debug, Error)]
pub enum EvidenceError {
    /// `repo_root` is not the top level of a git worktree.
    #[error("not a git repository root: {0}")]
    NotGitRoot(String),
    /// The pinned revision does not exist in the repository.
    #[error("revision not found: {0}")]
    RevisionNotFound(String),
    /// A declared source failed verification.
    #[error("source '{path}' failed verification: {reason}")]
    SourceRejected {
        /// Declared repo-relative path.
        path: String,
        /// Why the declaration was rejected.
        reason: String,
    },
    /// A git subprocess failed to spawn or errored.
    #[error("git command failed: {0}")]
    Git(String),
}

/// One verified (or rejected) source reference.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VerifiedSource {
    /// Repo-relative path.
    pub path: String,
    /// Declared label, carried through for the viewer badge tooltip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// 1-based start line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Inclusive end line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    /// Web link to the pinned blob (GitHub-compatible URLs only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

/// Result of verifying a component's sources against a pinned revision.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EvidenceReport {
    /// True iff every declared source verified.
    pub verified: bool,
    /// Origin URL of the repository (when a remote exists).
    pub repository: Option<String>,
    /// Pinned revision (short form for display).
    pub revision: Option<String>,
    /// Verified references with blob links.
    pub references: Vec<VerifiedSource>,
}

/// Rejects repository-escaping or malformed relative paths before any git
/// invocation.
///
/// # Errors
///
/// Returns a human-readable reason for `..` segments, `.git` components,
/// backslashes, control characters, or empty paths.
pub fn sanitize_rel_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("empty path".to_string());
    }
    if path.contains('\\') {
        return Err("backslash in path".to_string());
    }
    if path.chars().any(char::is_control) {
        return Err("control character in path".to_string());
    }
    let mut segments = Path::new(path).components();
    for component in segments.by_ref() {
        match component {
            std::path::Component::Normal(name) => {
                if name == ".git" {
                    return Err(".git component in path".to_string());
                }
            }
            std::path::Component::ParentDir => {
                return Err(".. segment in path".to_string());
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err("absolute path".to_string());
            }
            std::path::Component::CurDir => {}
        }
    }
    Ok(())
}

fn run_git(repo_root: &Path, args: &[&str]) -> Result<String, EvidenceError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .map_err(|e| EvidenceError::Git(format!("spawn: {e}")))?;
    if !output.status.success() {
        return Err(EvidenceError::Git(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Builds a GitHub blob link when the origin URL is a github.com HTTPS URL.
fn blob_href(origin: &str, revision: &str, source: &SourceRef) -> Option<String> {
    let url = origin.trim_end_matches('/');
    let rest = url.strip_prefix("https://github.com/")?;
    if rest.is_empty() {
        return None;
    }
    let mut href = format!("{url}/blob/{revision}/{}", source.path);
    match (source.line, source.end_line) {
        (Some(line), Some(end)) => {
            href.push_str(&format!("#L{line}-L{end}"));
        }
        (Some(line), None) => href.push_str(&format!("#L{line}")),
        _ => {}
    }
    Some(href)
}

/// Verifies declared `sources` against `revision` in the git worktree at
/// `repo_root`. `expected_origin`, when non-empty, must match the remote.
///
/// # Errors
///
/// Returns [`EvidenceError`] when the root is not a git toplevel, the
/// revision is missing, or any source is rejected.
pub fn verify(
    repo_root: &Path,
    revision: &str,
    sources: &[SourceRef],
    expected_origin: &str,
) -> Result<EvidenceReport, EvidenceError> {
    if sources.is_empty() {
        return Ok(EvidenceReport {
            verified: true,
            repository: None,
            revision: Some(revision.to_string()),
            references: Vec::new(),
        });
    }

    let toplevel = run_git(repo_root, &["rev-parse", "--show-toplevel"])?;
    let toplevel_path = PathBuf::from(&toplevel);
    let canonical_root = repo_root
        .canonicalize()
        .map_err(|e| EvidenceError::Git(format!("canonicalize root: {e}")))?;
    if toplevel_path != canonical_root {
        return Err(EvidenceError::NotGitRoot(repo_root.display().to_string()));
    }

    run_git(
        repo_root,
        &["cat-file", "-e", &format!("{revision}^{{commit}}")],
    )
    .map_err(|_| EvidenceError::RevisionNotFound(revision.to_string()))?;

    let origin = run_git(repo_root, &["remote", "get-url", "origin"]).ok();
    if !expected_origin.is_empty()
        && origin.as_deref() != Some(expected_origin.trim_end_matches('/'))
    {
        return Err(EvidenceError::SourceRejected {
            path: "(origin)".to_string(),
            reason: format!("declared origin {expected_origin:?} does not match remote {origin:?}"),
        });
    }
    let origin_for_links = expected_origin
        .is_empty()
        .then_some(origin.clone())
        .flatten()
        .or_else(|| Some(expected_origin.to_string()).filter(|s| !s.is_empty()));

    let mut references = Vec::with_capacity(sources.len());
    for source in sources {
        sanitize_rel_path(&source.path).map_err(|reason| EvidenceError::SourceRejected {
            path: source.path.clone(),
            reason,
        })?;
        let object = format!("{}:{}", revision, source.path);
        let kind = run_git(repo_root, &["cat-file", "-t", &object]).map_err(|_| {
            EvidenceError::SourceRejected {
                path: source.path.clone(),
                reason: format!("missing at revision {revision}"),
            }
        })?;
        if kind != "blob" {
            return Err(EvidenceError::SourceRejected {
                path: source.path.clone(),
                reason: format!("not a blob ({kind})"),
            });
        }
        if source.line.is_some() || source.end_line.is_some() {
            let content = run_git(repo_root, &["show", &object])?;
            let line_count = content.lines().count() as u32;
            let end = source.end_line.or(source.line).unwrap_or(0);
            if end > line_count {
                return Err(EvidenceError::SourceRejected {
                    path: source.path.clone(),
                    reason: format!("line range ends at {end} but file has {line_count} lines"),
                });
            }
        }
        references.push(VerifiedSource {
            path: source.path.clone(),
            label: source.label.clone(),
            line: source.line,
            end_line: source.end_line,
            href: origin_for_links
                .as_deref()
                .and_then(|o| blob_href(o, revision, source)),
        });
    }

    Ok(EvidenceReport {
        verified: true,
        repository: origin,
        revision: Some(revision.to_string()),
        references,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(path: &str) -> SourceRef {
        SourceRef {
            path: path.to_string(),
            line: None,
            end_line: None,
            label: None,
        }
    }

    #[test]
    fn sanitize_rejects_escape_and_malformed_paths() {
        assert!(sanitize_rel_path("src/main.rs").is_ok());
        assert!(sanitize_rel_path("a/../..").is_err(), ".. must be rejected");
        assert!(
            sanitize_rel_path(".git/config").is_err(),
            ".git must be rejected"
        );
        assert!(
            sanitize_rel_path("src\\windows.rs").is_err(),
            "backslash must be rejected"
        );
        assert!(
            sanitize_rel_path("bad\u{7}path").is_err(),
            "control chars must be rejected"
        );
        assert!(sanitize_rel_path("").is_err());
        assert!(sanitize_rel_path("/abs/path").is_err());
    }

    /// Creates a real git repository with one committed file; returns the
    /// temp root (RAII-cleaned on drop) and the committed revision.
    fn init_repo_with_file() -> (tempfile::TempDir, PathBuf, String) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("src")).expect("mkdir");
        std::fs::write(root.join("src/lib.rs"), "line1\nline2\nline3\n").expect("write");
        let git = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .expect("git spawn")
        };
        assert!(git(&["init", "-q"]).status.success());
        assert!(
            git(&["-c", "user.email=t@t", "-c", "user.name=t", "add", "."])
                .status
                .success()
        );
        assert!(git(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "init"
        ])
        .status
        .success());
        let rev = {
            let out = git(&["rev-parse", "HEAD"]);
            assert!(out.status.success());
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        (dir, root, rev)
    }

    #[test]
    fn verify_accepts_committed_sources_with_line_ranges() {
        let (_guard, root, rev) = init_repo_with_file();
        let sources = vec![SourceRef {
            path: "src/lib.rs".to_string(),
            line: Some(1),
            end_line: Some(3),
            label: Some("entry".to_string()),
        }];
        let report = verify(&root, &rev, &sources, "").expect("verify should pass");
        assert!(report.verified);
        assert_eq!(report.references.len(), 1);
        assert!(report.references[0].href.is_none(), "no origin, no link");
    }

    #[test]
    fn verify_rejects_missing_files_and_out_of_range_lines() {
        let (_guard, root, rev) = init_repo_with_file();
        let missing = verify(&root, &rev, &[source("src/ghost.rs")], "");
        assert!(
            matches!(missing, Err(EvidenceError::SourceRejected { .. })),
            "{missing:?}"
        );

        let out_of_range = verify(
            &root,
            &rev,
            &[SourceRef {
                path: "src/lib.rs".to_string(),
                line: Some(1),
                end_line: Some(99),
                label: None,
            }],
            "",
        );
        assert!(matches!(
            out_of_range,
            Err(EvidenceError::SourceRejected { .. })
        ));
    }

    #[test]
    fn verify_rejects_unknown_revision() {
        let (_guard, root, _rev) = init_repo_with_file();
        let dead = "0".repeat(40);
        let err = verify(&root, &dead, &[source("src/lib.rs")], "");
        assert!(
            matches!(err, Err(EvidenceError::RevisionNotFound(_))),
            "{err:?}"
        );
    }
}
