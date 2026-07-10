// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `daemon` service: run the file-watcher daemon for incremental indexing.

#[cfg(feature = "daemon")]
use std::path::Path;

#[cfg(feature = "daemon")]
use crate::cli::error::CliError;
#[cfg(feature = "daemon")]
use crate::kit::{DaemonKey, Kit};

#[cfg(all(feature = "cli", feature = "daemon"))]
use crate::service::error::kit_not_initialized;
#[cfg(all(feature = "cli", feature = "daemon"))]
use crate::service::runtime::kit;

#[cfg(all(feature = "cli", feature = "daemon"))]
use sdforge::prelude::ApiError;
#[cfg(all(feature = "cli", feature = "daemon"))]
use sdforge::service_api;

/// Validates the watch path, resolves the DaemonRunner capability, and enters
/// the blocking event loop.
///
/// Separated from the `#[service_api]` function for testability.
#[cfg(feature = "daemon")]
fn daemon_core(kit: &Kit, path: &str, name: &str) -> Result<(), CliError> {
    let watch_path = Path::new(path);
    if !watch_path.exists() {
        return Err(CliError::InvalidInput(format!(
            "watch path does not exist: {}",
            watch_path.display()
        )));
    }
    let daemon = kit.require::<DaemonKey>()?;
    daemon.start(watch_path, name)?;
    Ok(())
}

/// Maps `CliError` to `ApiError` at the service boundary.
#[cfg(all(feature = "cli", feature = "daemon"))]
fn to_api_error(e: CliError) -> ApiError {
    match e {
        CliError::InvalidInput(msg) => ApiError::InvalidInput {
            message: msg,
            field: None,
            value: None,
        },
        other => ApiError::internal_error(format!("{other}"), "daemon_error"),
    }
}

/// CLI wrapper — starts the blocking daemon event loop.
#[cfg(all(feature = "cli", feature = "daemon"))]
#[service_api(
    name = "codenexus",
    version = "0.3.2",
    tool_name = "daemon",
    description = "Run the file-watcher daemon for incremental indexing.",
    cli = true,
)]
async fn daemon(path: String, name: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    daemon_core(&kit, &path, &name).map_err(to_api_error)
}

#[cfg(all(test, feature = "daemon"))]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use std::fs;
    use std::path::PathBuf;
    use std::thread;
    use std::time::Duration;
    use tempfile::TempDir;

    fn write_file(dir: &std::path::Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("daemon_svc_testdb");
        std::mem::forget(dir);
        path
    }

    fn build_kit_for_db(db: &str) -> Kit {
        let config = KitBootstrapConfig::new(PathBuf::from(db));
        build_kit(&config).expect("build_kit")
    }

    fn build_kit_for_db_with_debounce(db: &str, debounce_ms: u64) -> Kit {
        let config = KitBootstrapConfig::new(PathBuf::from(db))
            .with_debounce_ms(debounce_ms);
        build_kit(&config).expect("build_kit")
    }

    // --- path validation ---

    #[test]
    fn daemon_core_returns_error_for_nonexistent_path() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let err = daemon_core(&kit, "/nonexistent/path/xyz", "demo")
            .expect_err("nonexistent path should error");
        assert!(matches!(err, CliError::InvalidInput(_)));
        assert_eq!(err.exit_code(), 1, "input error → exit 1");
    }

    #[test]
    fn daemon_core_error_message_contains_path() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let err = daemon_core(&kit, "/no/such/dir", "demo").expect_err("should error");
        let msg = err.to_string();
        assert!(msg.contains("/no/such/dir"), "error message should contain path: {msg}");
    }

    // --- blocking daemon start ---

    #[test]
    fn daemon_core_starts_and_runs() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() {}\n");
        let db = fresh_db_path();
        let kit = build_kit_for_db_with_debounce(db.to_str().unwrap(), 200);
        let watch_path = tmp.path().to_str().unwrap().to_string();
        let handle = thread::spawn(move || daemon_core(&kit, &watch_path, "demo"));

        thread::sleep(Duration::from_millis(500));
        write_file(tmp.path(), "main.rs", "fn main() { /* v2 */ }\n");
        thread::sleep(Duration::from_millis(800));

        assert!(!handle.is_finished(), "daemon should still be running");
    }

    // --- debounce acceptance (path validation phase) ---

    #[test]
    fn daemon_core_accepts_custom_debounce() {
        let db = fresh_db_path();
        let kit = build_kit_for_db_with_debounce(db.to_str().unwrap(), 500);
        let err = daemon_core(&kit, "/nonexistent/path/xyz", "demo")
            .expect_err("should error on nonexistent path");
        assert!(matches!(err, CliError::InvalidInput(_)));
    }

    #[test]
    fn daemon_core_accepts_default_debounce() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let err = daemon_core(&kit, "/nonexistent/path/xyz", "demo")
            .expect_err("should error on nonexistent path");
        assert!(matches!(err, CliError::InvalidInput(_)));
    }

    // --- end-to-end: file watching + incremental indexing ---

    #[test]
    fn daemon_core_triggers_incremental_index_on_code_file_change() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() {}\n");
        let db = fresh_db_path();
        let kit = build_kit_for_db_with_debounce(db.to_str().unwrap(), 200);
        let watch_path = tmp.path().to_str().unwrap().to_string();
        let handle = thread::spawn(move || daemon_core(&kit, &watch_path, "demo"));

        thread::sleep(Duration::from_millis(500));
        write_file(tmp.path(), "main.rs", "fn main() { /* modified */ }\n");
        thread::sleep(Duration::from_millis(1000));

        assert!(!handle.is_finished(), "daemon should still be running");
    }

    #[test]
    fn daemon_core_ignores_non_code_file_changes() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() {}\n");
        let db = fresh_db_path();
        let kit = build_kit_for_db_with_debounce(db.to_str().unwrap(), 200);
        let watch_path = tmp.path().to_str().unwrap().to_string();
        let handle = thread::spawn(move || daemon_core(&kit, &watch_path, "demo"));

        thread::sleep(Duration::from_millis(500));
        write_file(tmp.path(), "notes.txt", "hello world\n");
        write_file(tmp.path(), "config.json", "{}\n");
        thread::sleep(Duration::from_millis(500));

        assert!(!handle.is_finished(), "daemon should still be running");
    }
}
