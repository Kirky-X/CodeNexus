// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! File-watching daemon (Observer pattern).
//!
//! Uses [`notify_debouncer_full`] (ADR-013) to watch repositories and trigger
//! incremental indexing with configurable debounce (BR-DAEMON-001/004).
//!
//! # 状态机（ADD §4.2）
//!
//! ```text
//! [*] --> 监视中: codenexus daemon
//! 监视中 --> 待处理: 文件变更事件
//! 待处理 --> 待处理: 新事件（重置防抖）
//! 待处理 --> 索引中: 防抖窗口结束
//! 索引中 --> 监视中: 增量索引完成
//! 监视中 --> [*]: 用户中断
//! 索引中 --> 监视中: 索引失败（记录日志）
//! ```
//!
//! # 观察者模式
//!
//! [`Daemon`] 是主题（Subject），持有一组 [`EventObserver`]。当防抖窗口
//! 结束并收到一批代码文件变更事件后，Daemon 将过滤后的事件通知所有观察者。
//! [`IndexObserver`] 是内置观察者，收到事件后调用 [`IndexFacade::index_incremental`]
//! 触发增量索引。
//!
//! # trait-kit integration (Task 2.11)
//!
//! When the `daemon` feature is enabled, [`capability::DaemonRunner`] is the
//! capability trait stored in [`Kit`](crate::kit::Kit) under
//! [`DaemonKey`](crate::kit::DaemonKey). The concrete impl
//! ([`module::DaemonCapability`]) wraps the existing [`Daemon`] +
//! [`IndexObserver`] so the unified Kit can hand a pre-configured daemon
//! handle to `daemon_cmd::run` instead of having the CLI construct
//! subsystems ad-hoc.

pub mod capability;
pub mod daemon;
pub mod error;
pub mod event;
pub mod index_observer;
pub mod module;

pub use capability::DaemonRunner;
pub use daemon::{Daemon, DEFAULT_DEBOUNCE_MS};
pub use error::DaemonError;
pub use event::{DaemonEvent, EventObserver};
pub use index_observer::IndexObserver;
pub use module::{DaemonConfig, DaemonModule, DaemonModuleBuilder};
