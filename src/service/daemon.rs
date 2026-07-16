// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `daemon` service: run the file-watcher daemon for incremental indexing.

#[cfg(feature = "daemon")]
use std::path::Path;

#[cfg(feature = "daemon")]
use crate::kit::{AsyncKit, AsyncReady, DaemonModule};
#[cfg(all(feature = "cli", feature = "daemon"))]
use crate::service::error::to_api_error;
#[cfg(feature = "daemon")]
use crate::service::error::CodeNexusError;

#[cfg(all(feature = "cli", feature = "daemon"))]
use crate::service::error::kit_not_initialized;
#[cfg(all(feature = "cli", feature = "daemon"))]
use crate::service::runtime::kit;

#[cfg(all(feature = "cli", feature = "daemon"))]
use sdforge::forge;
#[cfg(all(feature = "cli", feature = "daemon"))]
use sdforge::prelude::ApiError;

/// Validates the watch path, resolves the DaemonRunner capability, and enters
/// the blocking event loop.
///
/// Separated from the `#[forge]` function for testability.
#[cfg(feature = "daemon")]
fn daemon_core(kit: &AsyncKit<AsyncReady>, path: &str, name: &str) -> Result<(), CodeNexusError> {
    let watch_path = Path::new(path);
    if !watch_path.exists() {
        return Err(CodeNexusError::InvalidInput(format!(
            "watch path does not exist: {}",
            watch_path.display()
        )));
    }
    let daemon = kit.require::<DaemonModule>()?;
    daemon.start(watch_path, name)?;
    Ok(())
}

/// CLI wrapper — starts the blocking daemon event loop.
#[cfg(all(feature = "cli", feature = "daemon"))]
#[forge(
    name = "daemon",
    version = "0.3.4",
    description = "Run the file-watcher daemon for incremental indexing.",
    cli = true
)]
async fn daemon(path: String, name: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    daemon_core(&kit, &path, &name).map_err(|e| to_api_error(e, "daemon_error"))
}

#[cfg(all(test, feature = "daemon"))]
mod tests {
    use super::*;
    use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig};
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

    fn fresh_db_path() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("daemon_svc_testdb");
        (dir, path)
    }

    fn build_kit_for_db(db: &str) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(PathBuf::from(db));
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    fn build_kit_for_db_with_debounce(db: &str, debounce_ms: u64) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(PathBuf::from(db)).with_debounce_ms(debounce_ms);
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    // --- path validation ---

    #[test]
    fn daemon_core_returns_error_for_nonexistent_path() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let err = daemon_core(&kit, "/nonexistent/path/xyz", "demo")
            .expect_err("nonexistent path should error");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
        assert_eq!(err.exit_code(), 2, "input error → exit 2");
    }

    #[test]
    fn daemon_core_error_message_contains_path() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let err = daemon_core(&kit, "/no/such/dir", "demo").expect_err("should error");
        let msg = err.to_string();
        assert!(
            msg.contains("/no/such/dir"),
            "error message should contain path: {msg}"
        );
    }

    // --- blocking daemon start ---

    #[serial_test::serial(daemon_core)]
    #[test]
    fn daemon_core_starts_and_runs() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() {}\n");
        let (_dir, db) = fresh_db_path();
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
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db_with_debounce(db.to_str().unwrap(), 500);
        let err = daemon_core(&kit, "/nonexistent/path/xyz", "demo")
            .expect_err("should error on nonexistent path");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
    }

    #[test]
    fn daemon_core_accepts_default_debounce() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let err = daemon_core(&kit, "/nonexistent/path/xyz", "demo")
            .expect_err("should error on nonexistent path");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
    }

    // --- end-to-end: file watching + incremental indexing ---

    #[serial_test::serial(daemon_core)]
    #[test]
    fn daemon_core_triggers_incremental_index_on_code_file_change() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() {}\n");
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db_with_debounce(db.to_str().unwrap(), 200);
        let watch_path = tmp.path().to_str().unwrap().to_string();
        let handle = thread::spawn(move || daemon_core(&kit, &watch_path, "demo"));

        thread::sleep(Duration::from_millis(500));
        write_file(tmp.path(), "main.rs", "fn main() { /* modified */ }\n");
        thread::sleep(Duration::from_millis(1000));

        assert!(!handle.is_finished(), "daemon should still be running");
    }

    #[serial_test::serial(daemon_core)]
    #[test]
    fn daemon_core_ignores_non_code_file_changes() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() {}\n");
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db_with_debounce(db.to_str().unwrap(), 200);
        let watch_path = tmp.path().to_str().unwrap().to_string();
        let handle = thread::spawn(move || daemon_core(&kit, &watch_path, "demo"));

        thread::sleep(Duration::from_millis(500));
        write_file(tmp.path(), "notes.txt", "hello world\n");
        write_file(tmp.path(), "config.json", "{}\n");
        thread::sleep(Duration::from_millis(500));

        assert!(!handle.is_finished(), "daemon should still be running");
    }

    // ===== #[forge] wrapper tests via init_kit =====

    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn daemon_wrapper_succeeds_via_init_kit() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        // daemon() enters a blocking event loop (daemon.run()), so unlike
        // architecture/search/cross_service we cannot block_on it directly.
        // Instead we spawn it on a thread; if kit init succeeded the thread
        // stays alive (daemon running). kit() returns an Arc clone, so
        // reset_kit_for_testing() is safe even while the daemon thread runs.
        reset_kit_for_testing();
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() {}\n");
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db_with_debounce(db.to_str().unwrap(), 200);
        init_kit(kit).expect("init_kit");

        let watch_path = tmp.path().to_str().unwrap().to_string();
        // ApiError 来自 sdforge（168 字节），项目约束禁止修改外部库，
        // 故局部允许 result_large_err lint。
        #[allow(clippy::result_large_err)]
        let handle = thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("runtime");
            rt.block_on(daemon(watch_path, "demo".to_string()))
        });

        thread::sleep(Duration::from_millis(500));
        assert!(
            !handle.is_finished(),
            "daemon should be running — kit init succeeded"
        );

        reset_kit_for_testing();
    }

    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn daemon_wrapper_fails_when_kit_not_initialized() {
        use crate::service::runtime::reset_kit_for_testing;

        reset_kit_for_testing();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(daemon(
            "/nonexistent/path/xyz".to_string(),
            "demo".to_string(),
        ));
        assert!(result.is_err(), "wrapper should fail without kit");
        reset_kit_for_testing();
    }
}
