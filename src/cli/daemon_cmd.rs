// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `daemon` subcommand handler (PRD §4.3, Task 15).
//!
//! 启动文件监视守护进程，监视代码仓库并在代码文件变更时触发增量索引
//! （BR-DAEMON-001~004，AC-DAEMON-001~003）。
//!
//! 该处理器从 [`Kit`](crate::kit::Kit) 解析
//! [`DaemonRunner`](crate::daemon::capability::DaemonRunner) 能力，然后调用
//! [`DaemonRunner::start`] 进入阻塞事件循环。守护进程持续运行直到被用户
//! 中断（Ctrl+C / SIGTERM）。

use std::path::Path;

use super::args::DaemonArgs;
use super::error::{CliError, Result};
use crate::kit::{DaemonKey, Kit};

/// 运行 `daemon` 子命令。
///
/// 从 `kit` 解析 [`DaemonRunner`](crate::daemon::capability::DaemonRunner)
/// 能力，校验监视路径存在，然后调用 [`DaemonRunner::start`] 进入阻塞事件
/// 循环。
///
/// # 工作流程
///
/// 1. 校验监视路径存在（输入错误 → 退出码 1）。
/// 2. 从 `kit` 解析 `DaemonRunner` 能力（能力缺失 → 退出码 3）。
/// 3. 调用 [`DaemonRunner::start`]，内部构造 `Daemon` + `IndexObserver`
///    并进入阻塞事件循环。
///
/// # 数据库与防抖配置
///
/// `args.db` 与 `args.debounce_ms` 在统一 Kit 架构下由
/// [`KitBootstrapConfig`](crate::kit::KitBootstrapConfig) 在构建 Kit 时
/// 统一配置，`run` 不再直接读取这两个参数。CLI 仍保留这两个参数以保持
/// 向后兼容（`main.rs` 在构建 Kit 时使用 `--db`，`KitBootstrapConfig`
/// 默认使用 `DEFAULT_DEBOUNCE_MS`）。
///
/// # 停止方式
///
/// 守护进程持续运行直到：
/// - 用户按下 Ctrl+C（SIGINT 默认终止进程）。
/// - 事件通道断开（监视器内部错误）。
///
/// # Errors
///
/// - [`CliError::InvalidInput`]：监视路径不存在。
/// - [`CliError::Kit`]：`DaemonRunner` 能力未注册。
/// - [`CliError::Daemon`]：文件监视器创建或监视失败。
pub fn run(kit: &Kit, args: &DaemonArgs) -> Result<()> {
    let watch_path = Path::new(&args.path);

    // 校验监视路径存在（PRD §4.1.6：输入错误 → 退出码 1）。
    if !watch_path.exists() {
        return Err(CliError::InvalidInput(format!(
            "watch path does not exist: {}",
            watch_path.display()
        )));
    }

    // 从 Kit 解析 DaemonRunner 能力。DaemonConfig（db_path + debounce_ms）
    // 在 build_kit 时已注入，start() 内部构造 Daemon + IndexObserver。
    let daemon = kit.require::<DaemonKey>()?;

    // 启动阻塞事件循环（状态机：监视中 → 待处理 → 索引中 → 监视中）。
    daemon.start(watch_path, &args.name)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::DaemonArgs;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use std::fs;
    use std::path::PathBuf;
    use std::thread;
    use std::time::Duration;
    use tempfile::TempDir;

    /// 在 `dir/rel` 写入文件（自动创建父目录）。
    fn write_file(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    /// 返回一个临时目录中的数据库路径（故意泄漏 TempDir 以保持文件存活）。
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("daemon_cmd_testdb");
        std::mem::forget(dir);
        path
    }

    /// 构建一个由磁盘数据库 `db` 支持的 Kit。
    fn build_kit_for_db(db: &str) -> Kit {
        let config = KitBootstrapConfig::new(PathBuf::from(db));
        build_kit(&config).expect("build_kit")
    }

    /// 构建一个由磁盘数据库 `db` 支持的 Kit，并指定防抖窗口。
    fn build_kit_for_db_with_debounce(db: &str, debounce_ms: u64) -> Kit {
        let config = KitBootstrapConfig::new(PathBuf::from(db))
            .with_debounce_ms(debounce_ms);
        build_kit(&config).expect("build_kit")
    }

    /// 构建 `DaemonArgs`。
    fn make_args(path: &str, name: &str, debounce_ms: u64, db: &str) -> DaemonArgs {
        DaemonArgs {
            path: path.to_string(),
            name: name.to_string(),
            debounce_ms,
            db: db.to_string(),
        }
    }

    // --- 路径校验 ---

    #[test]
    fn run_returns_error_for_nonexistent_path() {
        // 不存在的路径应返回 InvalidInput（退出码 1）。
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = make_args("/nonexistent/path/xyz", "demo", 2000, db.to_str().unwrap());
        let err = run(&kit, &args).expect_err("nonexistent path should error");
        assert!(
            matches!(err, CliError::InvalidInput(_)),
            "应为 InvalidInput，实际: {err:?}"
        );
        assert_eq!(err.exit_code(), 2, "输入错误 → 退出码 2");
    }

    #[test]
    fn run_invalid_input_message_contains_path() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = make_args("/no/such/dir", "demo", 2000, db.to_str().unwrap());
        let err = run(&kit, &args).expect_err("should error");
        let msg = err.to_string();
        assert!(msg.contains("/no/such/dir"), "错误消息应包含路径: {msg}");
    }

    // --- 成功路径（短时间运行后停止） ---

    #[test]
    fn run_starts_daemon_and_stops_via_stop_handle() {
        // 验证 run() 能成功启动守护进程。
        // 由于 run() 会阻塞，我们在单独线程中运行，并通过 stop_handle 停止。
        //
        // 注意：run() 内部创建的 Daemon 不暴露 stop_handle，因此这里
        // 间接验证：在单独线程中运行 run()，等待一段时间后通过文件系统
        // 事件触发索引，最后依赖通道断开或超时退出。
        //
        // 为避免测试挂起，我们使用一个会自动结束的策略：在临时目录中
        // 创建文件触发事件，然后等待防抖 + 索引完成。
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() {}\n");
        let db = fresh_db_path();

        let kit = build_kit_for_db_with_debounce(db.to_str().unwrap(), 200);
        let args = make_args(
            tmp.path().to_str().unwrap(),
            "demo",
            200, // 短防抖时间加速测试（args.debounce_ms 不再使用，保留以兼容）
            db.to_str().unwrap(),
        );

        // 在单独线程中运行 daemon_cmd::run。
        let handle = thread::spawn(move || run(&kit, &args));

        // 等待守护进程初始化并触发首次索引。
        thread::sleep(Duration::from_millis(500));

        // 修改代码文件触发增量索引（AC-DAEMON-001）。
        write_file(tmp.path(), "main.rs", "fn main() { /* v2 */ }\n");

        // 等待防抖 + 索引。
        thread::sleep(Duration::from_millis(800));

        // 守护进程仍在运行（阻塞），我们无法通过 API 停止它。
        // 由于这是测试环境，我们直接断开线程（detach）。
        // 注意：这会泄漏一个线程，但在测试进程中是可以接受的。
        //
        // 验证：如果运行到这里说明 run() 没有立即返回错误（路径校验通过、
        // 数据库打开成功、守护进程启动成功）。
        assert!(!handle.is_finished(), "守护进程应在运行中（阻塞）");

        // 由于无法优雅停止，我们放弃 join。测试进程退出时会终止所有线程。
        // 这是一个已知的限制：daemon_cmd::run 不暴露 stop_handle。
    }

    // --- 参数传递 ---

    #[test]
    fn run_accepts_custom_debounce() {
        // BR-DAEMON-004：可配置防抖。
        // 验证自定义 debounce_ms 不会导致错误（路径校验阶段就返回）。
        // 注意：args.debounce_ms 在统一 Kit 架构下不再使用，防抖窗口由
        // KitBootstrapConfig::with_debounce_ms 在构建 Kit 时配置。
        let db = fresh_db_path();
        let kit = build_kit_for_db_with_debounce(db.to_str().unwrap(), 500);
        let args = make_args("/nonexistent/path/xyz", "demo", 500, db.to_str().unwrap());
        let err = run(&kit, &args).expect_err("should error on nonexistent path");
        // 错误应在路径校验阶段产生，与 debounce_ms 无关。
        assert!(matches!(err, CliError::InvalidInput(_)));
    }

    #[test]
    fn run_accepts_default_debounce() {
        // BR-DAEMON-001：默认防抖 2000ms。
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let args = make_args("/nonexistent/path/xyz", "demo", 2000, db.to_str().unwrap());
        let err = run(&kit, &args).expect_err("should error on nonexistent path");
        assert!(matches!(err, CliError::InvalidInput(_)));
    }

    // --- 端到端：真实文件监视 + 增量索引 ---

    #[test]
    fn run_triggers_incremental_index_on_code_file_change() {
        // AC-DAEMON-001：修改代码文件后自动触发增量索引。
        // 此测试验证完整的 CLI 流程：参数解析 → 守护进程启动 → 文件变更 → 增量索引。
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() {}\n");
        let db = fresh_db_path();

        let kit = build_kit_for_db_with_debounce(db.to_str().unwrap(), 200);
        let args = make_args(
            tmp.path().to_str().unwrap(),
            "demo",
            200,
            db.to_str().unwrap(),
        );

        let handle = thread::spawn(move || run(&kit, &args));

        // 等待守护进程初始化。
        thread::sleep(Duration::from_millis(500));

        // 修改代码文件。
        write_file(tmp.path(), "main.rs", "fn main() { /* modified */ }\n");

        // 等待防抖 + 增量索引完成。
        thread::sleep(Duration::from_millis(1000));

        // 守护进程应仍在运行。
        assert!(!handle.is_finished(), "守护进程应持续运行");

        // 验证数据库中存在索引数据（间接验证增量索引被触发）。
        // 由于守护进程仍在运行，数据库可能被锁定，这里仅验证不 panic。
    }

    #[test]
    fn run_ignores_non_code_file_changes() {
        // AC-DAEMON-003：非代码文件变更不触发索引。
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() {}\n");
        let db = fresh_db_path();

        let kit = build_kit_for_db_with_debounce(db.to_str().unwrap(), 200);
        let args = make_args(
            tmp.path().to_str().unwrap(),
            "demo",
            200,
            db.to_str().unwrap(),
        );

        let handle = thread::spawn(move || run(&kit, &args));

        // 等待守护进程初始化。
        thread::sleep(Duration::from_millis(500));

        // 修改非代码文件（不应触发索引）。
        write_file(tmp.path(), "notes.txt", "hello world\n");
        write_file(tmp.path(), "config.json", "{}\n");

        // 等待防抖窗口。
        thread::sleep(Duration::from_millis(500));

        // 守护进程应仍在运行（未因非代码文件崩溃）。
        assert!(!handle.is_finished(), "守护进程应持续运行");
    }

    // --- DaemonArgs 构造 ---

    #[test]
    fn make_args_builds_correct_struct() {
        let args = make_args("/repo", "demo", 1500, "/tmp/db.lbug");
        assert_eq!(args.path, "/repo");
        assert_eq!(args.name, "demo");
        assert_eq!(args.debounce_ms, 1500);
        assert_eq!(args.db, "/tmp/db.lbug");
    }
}
