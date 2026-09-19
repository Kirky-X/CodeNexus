// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Daemon event types and observer trait (Observer pattern).
use std::path::PathBuf;

use crate::index::IndexResult;

/// 文件变更事件（观察者模式中的主题数据）。
///
/// 表示经过防抖和代码文件过滤后，传递给观察者的变更事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonEvent {
    /// 文件创建事件。
    Create(PathBuf),
    /// 文件修改事件。
    Modify(PathBuf),
    /// 文件删除事件。
    Remove(PathBuf),
}

/// 观察者接口（观察者模式）。
///
/// 实现此 trait 的类型可以注册到 [`Daemon`](crate::daemon::Daemon)，在防抖窗口
/// 结束并收到一批代码文件变更事件后被通知。
pub trait EventObserver: Send {
    /// 处理一批文件变更事件。
    ///
    /// 实现者应在此方法中执行索引等耗时操作。错误应内部记录（日志），
    /// 不应 panic（符合状态机"索引失败 → 记录日志 → 回到监视中"）。
    fn on_events(&mut self, events: &[DaemonEvent]);

    /// 增量索引成功完成后的回调（默认空实现）。
    ///
    /// 仅关注文件事件的观察者（如 [`IndexObserver`] 自身）无需实现；
    /// 关注索引产物的观察者（如影响告警）覆写此方法。由
    /// [`IndexObserver`](crate::daemon::IndexObserver) 在
    /// `index_incremental` 返回 `Ok` 后按注册顺序回调。
    fn on_index_complete(&mut self, _result: &IndexResult, _changed_files: &[PathBuf]) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_event_equality() {
        let a = DaemonEvent::Create(PathBuf::from("foo.rs"));
        let b = DaemonEvent::Create(PathBuf::from("foo.rs"));
        let c = DaemonEvent::Modify(PathBuf::from("foo.rs"));
        let d = DaemonEvent::Create(PathBuf::from("bar.rs"));
        assert_eq!(a, b, "相同类型和路径应相等");
        assert_ne!(a, c, "不同类型应不等");
        assert_ne!(a, d, "不同路径应不等");
        assert_ne!(c, DaemonEvent::Remove(PathBuf::from("foo.rs")));
    }

    #[test]
    fn daemon_event_debug_format() {
        let e = DaemonEvent::Create(PathBuf::from("src/main.rs"));
        let s = format!("{e:?}");
        assert!(s.contains("Create"), "debug 应包含变体名: {s}");
        assert!(s.contains("src/main.rs"), "debug 应包含路径: {s}");

        let m = DaemonEvent::Modify(PathBuf::from("a.c"));
        assert!(format!("{m:?}").contains("Modify"));

        let r = DaemonEvent::Remove(PathBuf::from("b.py"));
        assert!(format!("{r:?}").contains("Remove"));
    }

    #[test]
    fn daemon_event_clone_is_equal() {
        let e = DaemonEvent::Modify(PathBuf::from("x.ts"));
        assert_eq!(e, e.clone());
    }
}
