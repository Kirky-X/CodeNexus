// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Daemon error types.

use thiserror::Error;

/// 守护模式错误类型。
#[derive(Debug, Error)]
pub enum DaemonError {
    /// 文件监视器（notify）错误。
    #[error("notify watcher error: {0}")]
    Notify(#[from] notify_debouncer_full::notify::Error),

    /// I/O 错误。
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// 信号注册错误（SIGTERM/SIGINT handler 安装失败）。
    #[error("signal registration failed: {0}")]
    Signal(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_error_notify_display() {
        let err = DaemonError::Notify(notify_debouncer_full::notify::Error::path_not_found());
        let msg = err.to_string();
        assert!(msg.contains("notify watcher error"), "got: {msg}");
    }

    #[test]
    fn daemon_error_io_display() {
        let err = DaemonError::Io(std::io::Error::other("disk full"));
        let msg = err.to_string();
        assert!(msg.contains("io error"), "got: {msg}");
        assert!(msg.contains("disk full"), "got: {msg}");
    }

    #[test]
    fn daemon_error_debug_includes_variant() {
        let err = DaemonError::Io(std::io::Error::other("x"));
        let s = format!("{err:?}");
        assert!(s.contains("Io"), "got: {s}");
    }

    #[test]
    fn daemon_error_from_io_error() {
        let io_err = std::io::Error::other("test");
        let err: DaemonError = io_err.into();
        assert!(matches!(err, DaemonError::Io(_)));
    }

    #[test]
    fn daemon_error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<DaemonError>();
    }
}
