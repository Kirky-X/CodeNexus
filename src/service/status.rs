// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Status command: list indexed projects with staleness check.

use std::path::{Path, PathBuf};

use serde::Serialize;

#[cfg(any(feature = "cli", test))]
use crate::kit::{AsyncKit, AsyncReady, StorageModule};
#[cfg(any(feature = "cli", test))]
use crate::service::error::CodeNexusError;
#[cfg(feature = "cli")]
use crate::service::error::{kit_not_initialized, to_api_error, wrap_error};
#[cfg(feature = "cli")]
use crate::service::runtime::kit;
#[cfg(any(feature = "cli", feature = "analysis", test))]
use crate::storage::StorageConfig;
use crate::storage::{is_table_missing_error, ProjectRecord};

#[cfg(feature = "cli")]
use sdforge::forge;
#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;

/// Flat status name of a healthy module (`HealthStatus::as_status_name`);
/// the aggregate payload exposes names only, so filtering compares names.
#[cfg(any(feature = "cli", test))]
const HEALTHY_STATUS: &str = "healthy";

/// JSON-serializable status output.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct StatusOutput {
    pub projects: Vec<ProjectOutput>,
    /// trait-kit module health (`health_aggregate()`), non-healthy modules as
    /// `name=status` entries. Empty when every module reports healthy (normal
    /// case) or when no health checker is registered.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kit_health: Vec<String>,
    /// Structured trait-kit health (`health_aggregate()`): worst-of overall
    /// status plus per-module entries carrying the checker detail. Absent
    /// when no health checker is registered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kit_health_aggregate: Option<KitHealthAggregateOutput>,
}

/// Local view of trait-kit's health aggregate
/// (`AsyncKit::health_aggregate()`).
///
/// The upstream type is serializable, but it has neither `PartialEq` nor
/// `Default` (both required by `StatusOutput`'s derives), and its field shape
/// is upstream-owned — this view pins the JSON contract (`status` / `healthy`
/// / `modules`) independently of trait-kit schema drift.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct KitHealthAggregateOutput {
    /// Worst-of overall status name (`healthy` / `degraded` / `unhealthy`).
    pub status: String,
    /// Convenience flag: `status == "healthy"`.
    pub healthy: bool,
    /// Per-module entries.
    pub modules: Vec<KitHealthModuleOutput>,
}

impl KitHealthAggregateOutput {
    /// Maps trait-kit's aggregate into the status payload. Returns `None`
    /// when the checker registry is empty (no health information), keeping
    /// normal output free of health noise.
    fn from_aggregate(aggregate: trait_kit::core::health::HealthAggregate) -> Option<Self> {
        if aggregate.modules.is_empty() {
            return None;
        }
        Some(Self {
            status: aggregate.status.to_owned(),
            healthy: aggregate.healthy,
            modules: aggregate
                .modules
                .into_iter()
                .map(|entry| KitHealthModuleOutput {
                    module: entry.module.to_owned(),
                    status: entry.status.to_owned(),
                    detail: entry.detail,
                })
                .collect(),
        })
    }
}

/// One per-module entry of [`KitHealthAggregateOutput`].
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct KitHealthModuleOutput {
    /// Module name.
    pub module: String,
    /// Flat status name (`healthy` / `degraded` / `unhealthy`).
    pub status: String,
    /// Checker detail for degraded / unhealthy states, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
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
#[cfg(any(feature = "cli", feature = "analysis", test))]
pub(crate) fn git_head_commit(root: &Path) -> String {
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

/// Computes staleness: true if both commits are non-empty and differ.
#[cfg(any(feature = "cli", feature = "analysis", test))]
pub(crate) fn is_stale(last_commit: &str, current_head: &str) -> bool {
    !last_commit.is_empty() && !current_head.is_empty() && last_commit != current_head
}

/// Resolves a project's root directory with fallback for legacy relative
/// `rootPath` values.
///
/// Older indexes stored `rootPath = "."` because the indexer didn't
/// canonicalize the path. When such an index is queried from a different
/// CWD, `git rev-parse HEAD` would run against the wrong directory and
/// produce false `is_stale=true` results. This function detects that case
/// and falls back to `db_path.parent().and_then(parent)` — the typical
/// layout is `<project>/.codenexus/<name>.lbug`, so two `.parent()` calls
/// walk from the DB file up to the project root.
///
/// Returns the original `root_path` (wrapped as `PathBuf`) when:
/// - it is already an absolute path that exists on disk, OR
/// - the fallback chain fails (no parent / no parent-of-parent or the
///   resolved path does not exist), in which case the caller will surface
///   the staleness as before (best-effort, no panic).
#[cfg(any(feature = "cli", feature = "analysis", test))]
pub(crate) fn resolve_project_root(root_path: &str, db_path: &Path) -> PathBuf {
    let p = Path::new(root_path);
    if p.is_absolute() && p.exists() {
        return p.to_path_buf();
    }
    db_path
        .parent()
        .and_then(|parent| parent.parent())
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .unwrap_or_else(|| p.to_path_buf())
}

/// Runs status against an injected Kit (testable core).
///
/// On a fresh/uninitialized DB (Project table missing), returns an empty
/// `StatusOutput` instead of erroring — the CLI `status` command should exit
/// 0 with a clean `{"projects":[]}` output. The "table missing" detection
/// lives at the service layer (not storage) so
/// [`QualityChecker::check_project_isolation`] keeps strict semantics for
/// violation detection.
#[cfg(any(feature = "cli", test))]
pub fn run_status(kit: &AsyncKit<AsyncReady>) -> Result<StatusOutput, CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let db_path = kit.config::<StorageConfig>()?.db_path.clone();
    let projects = match storage.list_projects() {
        Ok(projects) => projects,
        Err(err) if is_table_missing_error(&err) => return Ok(StatusOutput::default()),
        Err(err) => return Err(err.into()),
    };
    let aggregate = kit.health_aggregate();
    let output = StatusOutput {
        projects: projects
            .into_iter()
            .map(|p| {
                let root = resolve_project_root(&p.root_path, &db_path);
                let current_head = git_head_commit(&root);
                let stale = is_stale(&p.last_commit, &current_head);
                ProjectOutput::from_record(p, current_head, stale)
            })
            .collect(),
        kit_health: aggregate
            .modules
            .iter()
            .filter(|entry| entry.status != HEALTHY_STATUS)
            .map(|entry| format!("{}={}", entry.module, entry.status))
            .collect(),
        kit_health_aggregate: KitHealthAggregateOutput::from_aggregate(aggregate),
    };
    Ok(output)
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[forge::log(err_detail)]
#[forge(
    name = "status",
    version = "0.3.5",
    description = "List all indexed projects and check their staleness.",
    cli = true
)]
async fn status() -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let output = run_status(&kit).map_err(|e| to_api_error(e, "status_error"))?;
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, AsyncAutoBuilder, KitBootstrapConfig, ModuleMeta};
    use std::future::Future;
    use std::path::PathBuf;
    use std::pin::Pin;
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_status_testdb");
        (dir, path)
    }

    fn build_kit_for_db(db: &std::path::Path) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    /// Test-only module whose capability is never registered, so trait-kit's
    /// checker lookup reports `unhealthy` with a deterministic detail.
    struct MissingCapabilityProbe;

    impl ModuleMeta for MissingCapabilityProbe {
        const NAME: &'static str = "missing_probe";
    }

    impl AsyncAutoBuilder for MissingCapabilityProbe {
        type Capability = ();
        type Error = std::io::Error;

        fn build<'a>(
            _kit: &'a AsyncKit,
        ) -> Pin<Box<dyn Future<Output = Result<(), std::io::Error>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }

    impl trait_kit::core::health::AsyncHealthCheck for MissingCapabilityProbe {
        fn check(_cap: &()) -> trait_kit::core::health::HealthStatus {
            trait_kit::core::health::HealthStatus::Healthy
        }
    }

    /// Builds a minimal kit (StorageModule plus a deterministic failing probe)
    /// with health checkers registered before `build()` — checkers only take
    /// effect when registered on the unbuilt kit, the same pattern the
    /// production bootstrap uses for Storage/Cache.
    fn build_kit_with_checkers(db: &std::path::Path) -> AsyncKit<AsyncReady> {
        let mut kit = AsyncKit::new();
        kit.set_config(StorageConfig {
            db_path: db.to_path_buf(),
            read_only: false,
        });
        kit.register::<StorageModule>().expect("register storage");
        kit.register_health_check::<StorageModule>();
        kit.register_health_check::<MissingCapabilityProbe>();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(kit.build())
            .expect("build")
    }

    #[test]
    fn run_status_returns_empty_on_fresh_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_status(&kit).expect("run should succeed");
        assert!(output.projects.is_empty());
    }

    #[test]
    fn run_status_returns_empty_when_project_table_dropped() {
        // Simulates a corrupted/uninitialized DB where Project table is gone.
        // The service layer converts "table missing" errors into empty
        // StatusOutput so CLI exits 0 with `{"projects":[]}`.
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage
            .execute("DROP TABLE Project;")
            .expect("drop project table");
        let output = run_status(&kit).expect("run should succeed with empty output");
        assert!(
            output.projects.is_empty(),
            "dropped Project table should yield empty output"
        );
    }

    #[test]
    fn run_status_returns_project_with_staleness() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        storage
            .execute("CREATE (:Project {id: 'p1', name: 'demo', rootPath: '/nonexistent', language: 'rust', fileCount: 1, indexedAt: 1000, lastCommit: 'abc123'});")
            .expect("create project");
        let output = run_status(&kit).expect("run should succeed");
        assert_eq!(output.projects.len(), 1);
        let p = &output.projects[0];
        assert_eq!(p.id, "p1");
        assert_eq!(p.name, "demo");
        assert_eq!(p.last_commit, "abc123");
        // rootPath is not a git repo, so current_head is empty, stale is false.
        assert!(
            p.current_head.is_empty(),
            "non-git dir should have empty head"
        );
        assert!(!p.stale, "should not be stale when current_head is empty");
    }

    #[test]
    fn run_status_reports_health_from_production_kit() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let output = run_status(&kit).expect("run should succeed");
        // 生产 bootstrap 注册 storage（cache 开启时含 cache）checker：聚合必须
        // 存在、storage 健康；平面视图只列非健康模块（此处为空）。
        assert!(output.kit_health.is_empty());
        let aggregate = output
            .kit_health_aggregate
            .clone()
            .expect("production kit registers health checkers");
        assert_eq!(aggregate.status, "healthy");
        assert!(aggregate.healthy);
        assert!(
            aggregate
                .modules
                .iter()
                .any(|m| m.module == "storage" && m.status == "healthy"),
            "storage entry expected, got: {:?}",
            aggregate.modules
        );
        let json = serde_json::to_string(&output).unwrap();
        assert!(
            json.contains("kit_health_aggregate"),
            "aggregate field must be serialized: {json}"
        );
    }

    #[test]
    fn run_status_reports_worst_of_health_with_detail() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_with_checkers(&db);
        let output = run_status(&kit).expect("run should succeed");

        // Flat view keeps only the non-healthy module.
        assert_eq!(output.kit_health, vec!["missing_probe=unhealthy"]);
        let aggregate = output.kit_health_aggregate.expect("aggregate present");
        assert_eq!(
            aggregate.status, "unhealthy",
            "worst-of must surface the failing module"
        );
        assert!(!aggregate.healthy);
        let probe = aggregate
            .modules
            .iter()
            .find(|entry| entry.module == "missing_probe")
            .expect("missing_probe entry");
        assert_eq!(probe.status, "unhealthy");
        assert_eq!(probe.detail.as_deref(), Some("capability not found"));
        let storage = aggregate
            .modules
            .iter()
            .find(|entry| entry.module == "storage")
            .expect("storage entry");
        assert_eq!(storage.status, "healthy");
        assert!(storage.detail.is_none());
    }

    #[test]
    fn is_stale_returns_false_when_both_empty() {
        assert!(!is_stale("", ""));
    }

    #[test]
    fn is_stale_returns_false_when_last_commit_empty() {
        assert!(!is_stale("", "abc"));
    }

    #[test]
    fn is_stale_returns_false_when_current_head_empty() {
        assert!(!is_stale("abc", ""));
    }

    #[test]
    fn is_stale_returns_true_when_commits_differ() {
        assert!(is_stale("abc", "def"));
    }

    #[test]
    fn is_stale_returns_false_when_commits_match() {
        assert!(!is_stale("abc", "abc"));
    }

    #[test]
    fn git_head_commit_returns_empty_for_non_git_dir() {
        let dir = TempDir::new().unwrap();
        let head = git_head_commit(dir.path());
        assert!(head.is_empty(), "non-git dir should return empty string");
    }

    #[test]
    fn project_output_from_record_maps_all_fields() {
        let rec = ProjectRecord {
            id: "p1".into(),
            name: "demo".into(),
            root_path: "/demo".into(),
            language: "rust".into(),
            file_count: 42,
            indexed_at: 1234567890,
            last_commit: "deadbeef".into(),
        };
        let out = ProjectOutput::from_record(rec, "cafebabe".into(), true);
        assert_eq!(out.id, "p1");
        assert_eq!(out.name, "demo");
        assert_eq!(out.root_path, "/demo");
        assert_eq!(out.language, "rust");
        assert_eq!(out.file_count, 42);
        assert_eq!(out.indexed_at, 1234567890);
        assert_eq!(out.last_commit, "deadbeef");
        assert_eq!(out.current_head, "cafebabe");
        assert!(out.stale);
    }

    #[test]
    fn kit_health_aggregate_output_maps_entries_and_empty() {
        let aggregate = trait_kit::core::health::HealthAggregate {
            status: "unhealthy",
            healthy: false,
            modules: vec![
                trait_kit::core::health::HealthModuleEntry {
                    module: "storage",
                    status: "unhealthy",
                    detail: Some("db gone".into()),
                },
                trait_kit::core::health::HealthModuleEntry {
                    module: "cache",
                    status: "healthy",
                    detail: None,
                },
            ],
        };
        let output = KitHealthAggregateOutput::from_aggregate(aggregate).expect("non-empty");
        assert_eq!(output.status, "unhealthy");
        assert!(!output.healthy);
        assert_eq!(output.modules.len(), 2);
        assert_eq!(output.modules[0].module, "storage");
        assert_eq!(output.modules[0].status, "unhealthy");
        assert_eq!(output.modules[0].detail.as_deref(), Some("db gone"));
        assert_eq!(output.modules[1].detail, None);

        let empty = trait_kit::core::health::HealthAggregate {
            status: "healthy",
            healthy: true,
            modules: Vec::new(),
        };
        assert!(
            KitHealthAggregateOutput::from_aggregate(empty).is_none(),
            "empty checker registry must not emit a health payload"
        );
    }

    #[test]
    fn status_output_serializes_to_json() {
        let output = StatusOutput {
            projects: vec![ProjectOutput {
                id: "p1".into(),
                name: "demo".into(),
                root_path: "/demo".into(),
                language: "rust".into(),
                file_count: 10,
                indexed_at: 1000,
                last_commit: "abc".into(),
                current_head: "def".into(),
                stale: true,
            }],
            kit_health: vec!["storage=unhealthy".into()],
            kit_health_aggregate: Some(KitHealthAggregateOutput {
                status: "unhealthy".into(),
                healthy: false,
                modules: vec![KitHealthModuleOutput {
                    module: "storage".into(),
                    status: "unhealthy".into(),
                    detail: Some("db gone".into()),
                }],
            }),
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"projects\""));
        assert!(json.contains("\"stale\":true"));
        assert!(json.contains("\"kit_health\""));
        assert!(json.contains("storage=unhealthy"));
        assert!(json.contains("\"kit_health_aggregate\""));
        assert!(json.contains("\"detail\":\"db gone\""));

        // Healthy kit (empty kit_health) omits the field entirely.
        let healthy = StatusOutput {
            kit_health: Vec::new(),
            ..Default::default()
        };
        let healthy_json = serde_json::to_string(&healthy).unwrap();
        assert!(!healthy_json.contains("kit_health"));
        assert!(json.contains("\"current_head\":\"def\""));
    }

    // ===== #[forge] wrapper tests via init_kit =====

    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn status_wrapper_succeeds_via_init_kit() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(status());
        assert!(result.is_ok(), "wrapper should succeed: {:?}", result.err());

        reset_kit_for_testing();
    }

    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn status_wrapper_fails_when_kit_not_initialized() {
        use crate::service::runtime::reset_kit_for_testing;

        reset_kit_for_testing();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(status());
        assert!(result.is_err(), "wrapper should fail without kit");
        reset_kit_for_testing();
    }

    /// Legacy indexes stored `rootPath = "."`. Without
    /// [`resolve_project_root`], `git rev-parse HEAD` would run in the
    /// process CWD (which might be a different git repo) and return the
    /// wrong commit, causing false `stale=true`. This test verifies the
    /// `db_path`-based fallback resolves the actual project root.
    #[test]
    fn test_status_resolves_relative_rootpath_via_db_path() {
        let project_root = TempDir::new().unwrap();
        let project_root_path = project_root.path().canonicalize().unwrap();

        let status = std::process::Command::new("git")
            .arg("init")
            .arg(&project_root_path)
            .status();
        if status.is_err() || !status.unwrap().success() {
            eprintln!("skipping test: git init failed");
            return;
        }
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&project_root_path)
                .args(args)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        std::fs::write(project_root_path.join("README.md"), "init\n").unwrap();
        if !git(&["add", "."])
            || !git(&[
                "-c",
                "user.email=t@t.com",
                "-c",
                "user.name=t",
                "commit",
                "-m",
                "init",
            ])
        {
            eprintln!("skipping test: git commit failed");
            return;
        }
        let head = std::process::Command::new("git")
            .arg("-C")
            .arg(&project_root_path)
            .arg("rev-parse")
            .arg("HEAD")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if head.is_empty() {
            eprintln!("skipping test: could not determine HEAD");
            return;
        }

        // Create DB at <project_root>/.codenexus/test.lbug — the layout
        // `resolve_project_root`'s fallback expects.
        let db_dir = project_root_path.join(".codenexus");
        std::fs::create_dir_all(&db_dir).unwrap();
        let db_path = db_dir.join("test.lbug");

        let kit = build_kit_for_db(&db_path);
        let storage = kit.require::<StorageModule>().expect("storage");
        // rootPath deliberately set to "." (legacy). lastCommit = current HEAD
        // so stale should be false once the fallback resolves the root.
        storage
            .execute(&format!(
                "CREATE (:Project {{id: 'demo', name: 'demo', rootPath: '.', language: 'rust', fileCount: 1, indexedAt: 1000, lastCommit: '{head}'}});"
            ))
            .expect("create project");

        let output = run_status(&kit).expect("run_status");
        assert_eq!(output.projects.len(), 1, "exactly one project");
        let p = &output.projects[0];
        assert_eq!(
            p.current_head, head,
            "current_head must be the project's actual HEAD, not the CWD's HEAD"
        );
        assert!(
            !p.stale,
            "should not be stale: last_commit == current_head after fallback resolution"
        );
    }
}
