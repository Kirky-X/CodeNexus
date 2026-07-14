// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! File-watching daemon (Observer pattern subject).
//!
//! Uses [`notify_debouncer_full`] (ADR-013) to watch repositories and trigger
//! incremental indexing with configurable debounce (BR-DAEMON-001/004).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify_debouncer_full::notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, DebouncedEvent};
use tracing::{info, warn};

use crate::daemon::error::DaemonError;
use crate::daemon::event::{DaemonEvent, EventObserver};
use crate::discover::is_code_file;

/// 默认防抖窗口（毫秒），BR-DAEMON-001。
pub const DEFAULT_DEBOUNCE_MS: u64 = 2000;

/// 定时检查间隔（毫秒），PRD §4.3.2 步骤 6。
const TICK_INTERVAL_MS: u64 = 500;

/// 守护进程（观察者模式中的主题）。
///
/// 监视项目根目录，在防抖窗口结束后将过滤后的文件变更事件通知所有
/// 注册的观察者。
pub struct Daemon {
    /// 监视的项目根目录。
    watch_path: PathBuf,
    /// 项目名称。
    project_name: String,
    /// 防抖窗口（毫秒），BR-DAEMON-001/004。
    debounce_ms: u64,
    /// 数据库路径。
    db_path: PathBuf,
    /// 已注册的观察者列表。
    observers: Vec<Box<dyn EventObserver + Send>>,
    /// 停止标志（用于优雅关闭和测试）。
    stop: Arc<AtomicBool>,
}

impl Daemon {
    /// 创建一个新的守护进程实例。
    ///
    /// 创建后不会自动开始监视；需调用 [`run`](Self::run) 或
    /// [`run_for_duration`](Self::run_for_duration) 启动事件循环。
    #[must_use]
    pub fn new(
        watch_path: impl AsRef<Path>,
        project_name: impl Into<String>,
        debounce_ms: u64,
        db_path: impl AsRef<Path>,
    ) -> Self {
        Self {
            watch_path: watch_path.as_ref().to_path_buf(),
            project_name: project_name.into(),
            debounce_ms,
            db_path: db_path.as_ref().to_path_buf(),
            observers: Vec::new(),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 添加一个观察者。
    pub fn add_observer(&mut self, observer: Box<dyn EventObserver + Send>) {
        self.observers.push(observer);
    }

    /// 返回停止句柄，可用于从其他线程停止守护进程。
    #[must_use]
    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }

    /// 返回防抖窗口（毫秒）。
    #[must_use]
    pub fn debounce_ms(&self) -> u64 {
        self.debounce_ms
    }

    /// 返回监视路径。
    #[must_use]
    pub fn watch_path(&self) -> &Path {
        &self.watch_path
    }

    /// 返回项目名称。
    #[must_use]
    pub fn project_name(&self) -> &str {
        &self.project_name
    }

    /// 返回数据库路径。
    #[must_use]
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// 启动阻塞事件循环，直到调用 `stop_handle().store(true)` 或通道断开。
    ///
    /// # Errors
    ///
    /// 返回 [`DaemonError::Notify`] 如果无法创建防抖器或开始监视。
    pub fn run(&mut self) -> Result<(), DaemonError> {
        let (tx, rx) = mpsc::channel::<DebounceEventResult>();
        let mut debouncer = new_debouncer(Duration::from_millis(self.debounce_ms), None, tx)?;
        debouncer.watch(&self.watch_path, RecursiveMode::Recursive)?;

        info!(
            path = %self.watch_path.display(),
            project = %self.project_name,
            debounce_ms = self.debounce_ms,
            "守护模式已启动"
        );

        let tick = Duration::from_millis(TICK_INTERVAL_MS);
        loop {
            if self.stop.load(Ordering::SeqCst) {
                info!("收到停止信号，守护模式退出");
                break;
            }
            match rx.recv_timeout(tick) {
                Ok(Ok(events)) => self.process_debounced_events(&events),
                Ok(Err(errors)) => {
                    for err in &errors {
                        warn!(error = %err, "文件监视器错误");
                    }
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    warn!("事件通道已断开，守护模式退出");
                    break;
                }
            }
        }
        Ok(())
    }

    /// 运行事件循环指定时间后自动停止（用于测试）。
    ///
    /// # Errors
    ///
    /// 返回 [`DaemonError::Notify`] 如果无法创建防抖器或开始监视。
    pub fn run_for_duration(&mut self, duration: Duration) -> Result<(), DaemonError> {
        let (tx, rx) = mpsc::channel::<DebounceEventResult>();
        let mut debouncer = new_debouncer(Duration::from_millis(self.debounce_ms), None, tx)?;
        debouncer.watch(&self.watch_path, RecursiveMode::Recursive)?;

        let deadline = Instant::now() + duration;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let timeout = remaining.min(Duration::from_millis(TICK_INTERVAL_MS));
            match rx.recv_timeout(timeout) {
                Ok(Ok(events)) => self.process_debounced_events(&events),
                Ok(Err(errors)) => {
                    for err in &errors {
                        warn!(error = %err, "文件监视器错误");
                    }
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(())
    }

    /// 处理一批防抖后的事件：过滤非代码文件，通知所有观察者。
    fn process_debounced_events(&mut self, events: &[DebouncedEvent]) {
        let daemon_events: Vec<DaemonEvent> =
            events.iter().filter_map(Self::convert_event).collect();

        if daemon_events.is_empty() {
            return;
        }

        // LOG-005：记录每个 daemon 事件（在通知观察者之前）。
        for event in &daemon_events {
            let (change_type, path) = match event {
                DaemonEvent::Create(p) => ("create", p.display()),
                DaemonEvent::Modify(p) => ("modify", p.display()),
                DaemonEvent::Remove(p) => ("remove", p.display()),
            };
            info!(
                event = "daemon_event",
                change_type = change_type,
                path = %path,
                "daemon event"
            );
        }

        for observer in &mut self.observers {
            observer.on_events(&daemon_events);
        }
    }

    /// 将一个 [`DebouncedEvent`] 转换为 [`DaemonEvent`]，过滤非代码文件。
    fn convert_event(event: &DebouncedEvent) -> Option<DaemonEvent> {
        let path = event.paths.first()?;
        is_code_file(path)?;
        match event.kind {
            EventKind::Create(_) => Some(DaemonEvent::Create(path.clone())),
            EventKind::Modify(_) => Some(DaemonEvent::Modify(path.clone())),
            EventKind::Remove(_) => Some(DaemonEvent::Remove(path.clone())),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::index_observer::IndexObserver;
    use crate::index::IndexFacade;
    use crate::test_log_capture::capture_tracing;
    use notify_debouncer_full::notify::event::EventAttributes;
    use notify_debouncer_full::notify::Event;
    use std::fs;
    use std::sync::Mutex;
    use std::thread;
    use tempfile::TempDir;

    // --- 测试辅助函数 ---

    fn write_file(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn fresh_db_path() -> PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("daemon_testdb");
        std::mem::forget(dir);
        path
    }

    fn make_event(kind: EventKind, path: &str) -> DebouncedEvent {
        DebouncedEvent {
            event: Event {
                kind,
                paths: vec![PathBuf::from(path)],
                attrs: EventAttributes::new(),
            },
            time: Instant::now(),
        }
    }

    type CallCountRef = Arc<Mutex<usize>>;
    type EventsRef = Arc<Mutex<Vec<DaemonEvent>>>;

    struct CountingObserver {
        call_count: CallCountRef,
        events: EventsRef,
    }

    impl CountingObserver {
        fn new() -> (Self, CallCountRef, EventsRef) {
            let call_count = Arc::new(Mutex::new(0));
            let events = Arc::new(Mutex::new(Vec::new()));
            let observer = CountingObserver {
                call_count: Arc::clone(&call_count),
                events: Arc::clone(&events),
            };
            (observer, call_count, events)
        }
    }

    impl EventObserver for CountingObserver {
        fn on_events(&mut self, events: &[DaemonEvent]) {
            *self.call_count.lock().unwrap() += 1;
            self.events.lock().unwrap().extend(events.iter().cloned());
        }
    }

    struct SignalingCountingObserver {
        call_count: Arc<Mutex<usize>>,
        signal: Arc<Mutex<Option<std::sync::mpsc::Sender<()>>>>,
        stop: Arc<AtomicBool>,
    }

    impl SignalingCountingObserver {
        fn new(stop: Arc<AtomicBool>) -> (Self, Arc<Mutex<usize>>, std::sync::mpsc::Receiver<()>) {
            let call_count = Arc::new(Mutex::new(0));
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            let signal = Arc::new(Mutex::new(Some(tx)));
            let observer = SignalingCountingObserver {
                call_count: Arc::clone(&call_count),
                signal,
                stop,
            };
            (observer, call_count, rx)
        }
    }

    impl EventObserver for SignalingCountingObserver {
        fn on_events(&mut self, _events: &[DaemonEvent]) {
            *self.call_count.lock().unwrap() += 1;
            if let Some(tx) = self.signal.lock().unwrap().take() {
                self.stop.store(true, Ordering::SeqCst);
                let _ = tx.send(());
            }
        }
    }

    // --- Daemon::new ---

    #[test]
    fn daemon_new_creates_instance() {
        let daemon = Daemon::new("/repo", "demo", 2000, "/tmp/db.lbug");
        assert_eq!(daemon.watch_path(), Path::new("/repo"));
        assert_eq!(daemon.project_name(), "demo");
        assert_eq!(daemon.debounce_ms(), 2000);
        assert_eq!(daemon.db_path(), Path::new("/tmp/db.lbug"));
    }

    #[test]
    fn daemon_default_debounce_is_2000() {
        let daemon = Daemon::new("/repo", "demo", DEFAULT_DEBOUNCE_MS, "/tmp/db.lbug");
        assert_eq!(daemon.debounce_ms(), 2000);
    }

    #[test]
    fn daemon_respects_custom_debounce() {
        let daemon = Daemon::new("/repo", "demo", 500, "/tmp/db.lbug");
        assert_eq!(daemon.debounce_ms(), 500);
    }

    // --- Daemon::add_observer ---

    #[test]
    fn daemon_add_observer_stores_observer() {
        let mut daemon = Daemon::new("/repo", "demo", 2000, "/tmp/db.lbug");
        let (observer, call_count, events) = CountingObserver::new();
        daemon.add_observer(Box::new(observer));
        assert_eq!(*call_count.lock().unwrap(), 0);
        assert!(events.lock().unwrap().is_empty());
    }

    #[test]
    fn daemon_observer_trait_object_works() {
        let mut daemon = Daemon::new("/repo", "demo", 2000, "/tmp/db.lbug");
        let (observer, call_count, _events) = CountingObserver::new();
        daemon.add_observer(Box::new(observer));

        let debounced_events = vec![make_event(
            EventKind::Create(notify_debouncer_full::notify::event::CreateKind::File),
            "foo.rs",
        )];
        daemon.process_debounced_events(&debounced_events);
        assert_eq!(*call_count.lock().unwrap(), 1, "观察者应被调用一次");
    }

    // --- Daemon::convert_event ---

    #[test]
    fn convert_event_create_code_file() {
        let event = make_event(
            EventKind::Create(notify_debouncer_full::notify::event::CreateKind::File),
            "src/main.rs",
        );
        let result = Daemon::convert_event(&event);
        assert_eq!(
            result,
            Some(DaemonEvent::Create(PathBuf::from("src/main.rs")))
        );
    }

    #[test]
    #[cfg(feature = "lang-c")]
    fn convert_event_modify_code_file() {
        let event = make_event(
            EventKind::Modify(notify_debouncer_full::notify::event::ModifyKind::Any),
            "lib.c",
        );
        let result = Daemon::convert_event(&event);
        assert_eq!(result, Some(DaemonEvent::Modify(PathBuf::from("lib.c"))));
    }

    #[test]
    #[cfg(feature = "lang-python")]
    fn convert_event_remove_code_file() {
        let event = make_event(
            EventKind::Remove(notify_debouncer_full::notify::event::RemoveKind::File),
            "old.py",
        );
        let result = Daemon::convert_event(&event);
        assert_eq!(result, Some(DaemonEvent::Remove(PathBuf::from("old.py"))));
    }

    #[test]
    fn convert_event_filters_non_code_files() {
        let event = make_event(
            EventKind::Create(notify_debouncer_full::notify::event::CreateKind::File),
            "README.md",
        );
        assert_eq!(Daemon::convert_event(&event), None);

        let event = make_event(
            EventKind::Modify(notify_debouncer_full::notify::event::ModifyKind::Any),
            "config.ini",
        );
        assert_eq!(Daemon::convert_event(&event), None);

        let event = make_event(
            EventKind::Remove(notify_debouncer_full::notify::event::RemoveKind::File),
            "notes.txt",
        );
        assert_eq!(Daemon::convert_event(&event), None);
    }

    #[test]
    fn convert_event_filters_no_path() {
        let event = DebouncedEvent {
            event: Event {
                kind: EventKind::Create(notify_debouncer_full::notify::event::CreateKind::File),
                paths: vec![],
                attrs: EventAttributes::new(),
            },
            time: Instant::now(),
        };
        assert_eq!(Daemon::convert_event(&event), None);
    }

    #[test]
    fn convert_event_filters_other_event_kinds() {
        let event = make_event(EventKind::Any, "foo.rs");
        assert_eq!(Daemon::convert_event(&event), None);

        let event = make_event(EventKind::Other, "foo.rs");
        assert_eq!(Daemon::convert_event(&event), None);
    }

    #[test]
    fn convert_event_filters_access_events() {
        let event = make_event(
            EventKind::Access(notify_debouncer_full::notify::event::AccessKind::Any),
            "foo.rs",
        );
        assert_eq!(Daemon::convert_event(&event), None);
    }

    #[test]
    fn convert_event_modify_rust_file() {
        // Cover EventKind::Modify branch (line 211) without requiring
        // lang-c feature. The existing modify test uses "lib.c" which
        // is gated behind #[cfg(feature = "lang-c")].
        let event = make_event(
            EventKind::Modify(notify_debouncer_full::notify::event::ModifyKind::Any),
            "src/main.rs",
        );
        let result = Daemon::convert_event(&event);
        assert_eq!(
            result,
            Some(DaemonEvent::Modify(PathBuf::from("src/main.rs")))
        );
    }

    #[test]
    fn convert_event_remove_rust_file() {
        // Cover EventKind::Remove branch (line 212) without requiring
        // lang-python feature. The existing remove test uses "old.py"
        // which is gated behind #[cfg(feature = "lang-python")].
        let event = make_event(
            EventKind::Remove(notify_debouncer_full::notify::event::RemoveKind::File),
            "src/main.rs",
        );
        let result = Daemon::convert_event(&event);
        assert_eq!(
            result,
            Some(DaemonEvent::Remove(PathBuf::from("src/main.rs")))
        );
    }

    #[test]
    fn process_events_modify_and_remove_rust_files() {
        // Cover the Modify and Remove daemon_event logging branches
        // (lines 189-190) without requiring lang-c or lang-python features.
        let mut daemon = Daemon::new("/repo", "demo", 2000, "/tmp/db.lbug");
        let (observer, call_count, events) = CountingObserver::new();
        daemon.add_observer(Box::new(observer));

        let debounced_events = vec![
            make_event(
                EventKind::Modify(notify_debouncer_full::notify::event::ModifyKind::Any),
                "main.rs",
            ),
            make_event(
                EventKind::Remove(notify_debouncer_full::notify::event::RemoveKind::File),
                "util.rs",
            ),
        ];
        daemon.process_debounced_events(&debounced_events);

        assert_eq!(
            *call_count.lock().unwrap(),
            1,
            "observer should be called once"
        );
        let received = events.lock().unwrap();
        assert_eq!(received.len(), 2, "should receive 2 events");
        assert_eq!(received[0], DaemonEvent::Modify(PathBuf::from("main.rs")));
        assert_eq!(received[1], DaemonEvent::Remove(PathBuf::from("util.rs")));
    }

    // --- Daemon::process_debounced_events ---

    #[test]
    fn process_events_filters_non_code_files() {
        let mut daemon = Daemon::new("/repo", "demo", 2000, "/tmp/db.lbug");
        let (observer, call_count, events) = CountingObserver::new();
        daemon.add_observer(Box::new(observer));

        let debounced_events = vec![
            make_event(
                EventKind::Create(notify_debouncer_full::notify::event::CreateKind::File),
                "README.md",
            ),
            make_event(
                EventKind::Modify(notify_debouncer_full::notify::event::ModifyKind::Any),
                "config.ini",
            ),
        ];
        daemon.process_debounced_events(&debounced_events);

        assert_eq!(*call_count.lock().unwrap(), 0, "非代码文件不应触发观察者");
        assert!(events.lock().unwrap().is_empty());
    }

    #[test]
    #[cfg(feature = "lang-c")]
    fn process_events_notifies_observers_with_code_files() {
        let mut daemon = Daemon::new("/repo", "demo", 2000, "/tmp/db.lbug");
        let (observer, call_count, events) = CountingObserver::new();
        daemon.add_observer(Box::new(observer));

        let debounced_events = vec![
            make_event(
                EventKind::Create(notify_debouncer_full::notify::event::CreateKind::File),
                "main.rs",
            ),
            make_event(
                EventKind::Modify(notify_debouncer_full::notify::event::ModifyKind::Any),
                "lib.c",
            ),
        ];
        daemon.process_debounced_events(&debounced_events);

        assert_eq!(*call_count.lock().unwrap(), 1, "观察者应被调用一次");
        let received = events.lock().unwrap();
        assert_eq!(received.len(), 2, "应收到两个事件");
        assert_eq!(received[0], DaemonEvent::Create(PathBuf::from("main.rs")));
        assert_eq!(received[1], DaemonEvent::Modify(PathBuf::from("lib.c")));
    }

    #[test]
    fn process_events_empty_batch_does_not_notify() {
        let mut daemon = Daemon::new("/repo", "demo", 2000, "/tmp/db.lbug");
        let (observer, call_count, _events) = CountingObserver::new();
        daemon.add_observer(Box::new(observer));

        daemon.process_debounced_events(&[]);
        assert_eq!(*call_count.lock().unwrap(), 0, "空批次不应触发观察者");
    }

    #[test]
    fn process_events_all_filtered_does_not_notify() {
        let mut daemon = Daemon::new("/repo", "demo", 2000, "/tmp/db.lbug");
        let (observer, call_count, _events) = CountingObserver::new();
        daemon.add_observer(Box::new(observer));

        let debounced_events = vec![
            make_event(
                EventKind::Create(notify_debouncer_full::notify::event::CreateKind::File),
                "notes.txt",
            ),
            make_event(EventKind::Any, "foo.rs"),
        ];
        daemon.process_debounced_events(&debounced_events);
        assert_eq!(*call_count.lock().unwrap(), 0, "全部被过滤不应触发观察者");
    }

    #[test]
    fn process_events_notifies_multiple_observers() {
        let mut daemon = Daemon::new("/repo", "demo", 2000, "/tmp/db.lbug");
        let (obs1, count1, _events1) = CountingObserver::new();
        let (obs2, count2, _events2) = CountingObserver::new();
        daemon.add_observer(Box::new(obs1));
        daemon.add_observer(Box::new(obs2));

        let debounced_events = vec![make_event(
            EventKind::Create(notify_debouncer_full::notify::event::CreateKind::File),
            "main.rs",
        )];
        daemon.process_debounced_events(&debounced_events);

        assert_eq!(*count1.lock().unwrap(), 1, "观察者 1 应被调用");
        assert_eq!(*count2.lock().unwrap(), 1, "观察者 2 应被调用");
    }

    // --- 集成测试：真实文件监视 ---

    #[test]
    fn daemon_triggers_index_on_code_file_change() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() {}\n");

        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let observer = IndexObserver::new(facade, "demo".to_string(), tmp.path().to_path_buf());

        let mut daemon = Daemon::new(tmp.path(), "demo", 200, &db_path);
        daemon.add_observer(Box::new(observer));

        let stop = daemon.stop_handle();
        let handle = thread::spawn(move || daemon.run());

        thread::sleep(Duration::from_millis(400));
        write_file(tmp.path(), "main.rs", "fn main() { /* modified */ }\n");
        thread::sleep(Duration::from_millis(800));

        stop.store(true, Ordering::SeqCst);
        let result = handle.join().expect("thread should join");
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn daemon_run_for_duration_stops_after_timeout() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() {}\n");

        let db_path = fresh_db_path();
        let mut daemon = Daemon::new(tmp.path(), "demo", 200, &db_path);

        let start = Instant::now();
        let result = daemon.run_for_duration(Duration::from_millis(500));
        let elapsed = start.elapsed();

        assert!(
            result.is_ok(),
            "run_for_duration should succeed: {:?}",
            result.err()
        );
        assert!(
            elapsed >= Duration::from_millis(400),
            "应运行至少约 500ms，实际 {:?}",
            elapsed
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "不应运行过久，实际 {:?}",
            elapsed
        );
    }

    #[test]
    fn daemon_run_for_duration_catches_code_file_change() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() {}\n");

        let db_path = fresh_db_path();
        let (observer, call_count, events) = CountingObserver::new();

        let mut daemon = Daemon::new(tmp.path(), "demo", 200, &db_path);
        daemon.add_observer(Box::new(observer));

        let handle = thread::spawn(move || daemon.run_for_duration(Duration::from_secs(2)));

        thread::sleep(Duration::from_millis(400));
        write_file(tmp.path(), "main.rs", "fn main() { /* v2 */ }\n");

        let result = handle.join().expect("thread should join");
        assert!(
            result.is_ok(),
            "run_for_duration should succeed: {:?}",
            result.err()
        );

        let count = *call_count.lock().unwrap();
        assert!(
            count >= 1,
            "AC-DAEMON-001：修改代码文件应触发索引，实际调用次数: {count}"
        );
        let received = events.lock().unwrap();
        assert!(!received.is_empty(), "应收到至少一个事件");
    }

    #[test]
    fn daemon_run_for_duration_ignores_non_code_files() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() {}\n");

        let db_path = fresh_db_path();
        let (observer, _call_count, events) = CountingObserver::new();

        let mut daemon = Daemon::new(tmp.path(), "demo", 200, &db_path);
        daemon.add_observer(Box::new(observer));

        let handle = thread::spawn(move || daemon.run_for_duration(Duration::from_secs(2)));

        thread::sleep(Duration::from_millis(400));
        write_file(tmp.path(), "notes.txt", "hello world\n");
        thread::sleep(Duration::from_millis(100));
        write_file(tmp.path(), "main.rs", "fn main() { /* v2 */ }\n");

        let result = handle.join().expect("thread should join");
        assert!(result.is_ok());

        let received = events.lock().unwrap();
        let notes_in_events = received.iter().any(|e| match e {
            DaemonEvent::Create(p) | DaemonEvent::Modify(p) | DaemonEvent::Remove(p) => {
                p.to_string_lossy().contains("notes.txt")
            }
        });
        assert!(
            !notes_in_events,
            "AC-DAEMON-003：notes.txt 不应出现在事件中（非代码文件应被过滤），实际事件: {:?}",
            received
        );
    }

    #[test]
    fn daemon_merges_consecutive_changes() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");
        write_file(tmp.path(), "b.rs", "fn b() {}\n");

        let db_path = fresh_db_path();
        let mut daemon = Daemon::new(tmp.path(), "demo", 2000, &db_path);
        let stop = daemon.stop_handle();
        let (observer, call_count, signal_rx) = SignalingCountingObserver::new(Arc::clone(&stop));
        daemon.add_observer(Box::new(observer));

        let handle = thread::spawn(move || daemon.run());

        let stop_safety = Arc::clone(&stop);
        let safety_handle = thread::spawn(move || {
            thread::sleep(Duration::from_secs(10));
            stop_safety.store(true, Ordering::SeqCst);
        });

        thread::sleep(Duration::from_millis(500));

        for i in 0..3 {
            write_file(tmp.path(), "a.rs", &format!("fn a() {{ /* v{i} */ }}\n"));
            write_file(tmp.path(), "b.rs", &format!("fn b() {{ /* v{i} */ }}\n"));
            thread::sleep(Duration::from_millis(500));
        }

        match signal_rx.recv_timeout(Duration::from_secs(6)) {
            Ok(()) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                stop.store(true, Ordering::SeqCst);
                let _ = handle.join();
                panic!("AC-DAEMON-002：6 秒内未收到 on_events 信号，daemon 未触发索引");
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                stop.store(true, Ordering::SeqCst);
                let _ = handle.join();
                panic!("AC-DAEMON-002：信号通道断开，observer 未发送信号");
            }
        }

        let result = handle.join().expect("thread should join");
        assert!(result.is_ok(), "daemon 应正常停止: {:?}", result.err());

        drop(safety_handle);

        let count = *call_count.lock().unwrap();
        assert_eq!(
            count, 1,
            "AC-DAEMON-002：防抖应合并为单次索引，实际触发 {} 次",
            count
        );
    }

    #[test]
    fn daemon_run_stops_via_stop_handle() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() {}\n");

        let db_path = fresh_db_path();
        let mut daemon = Daemon::new(tmp.path(), "demo", 200, &db_path);
        let stop = daemon.stop_handle();

        let handle = thread::spawn(move || daemon.run());

        thread::sleep(Duration::from_millis(400));
        stop.store(true, Ordering::SeqCst);

        let result = handle.join().expect("thread should join");
        assert!(
            result.is_ok(),
            "run should stop cleanly: {:?}",
            result.err()
        );
    }

    #[test]
    fn daemon_run_returns_error_for_nonexistent_path() {
        let db_path = fresh_db_path();
        let mut daemon = Daemon::new("/nonexistent/path/xyz", "demo", 200, &db_path);
        let result = daemon.run();
        assert!(result.is_err(), "不存在的路径应返回错误");
        assert!(
            matches!(result.unwrap_err(), DaemonError::Notify(_)),
            "应为 Notify 错误"
        );
    }

    #[test]
    fn daemon_run_for_duration_returns_error_for_nonexistent_path() {
        let db_path = fresh_db_path();
        let mut daemon = Daemon::new("/nonexistent/path/xyz", "demo", 200, &db_path);
        let result = daemon.run_for_duration(Duration::from_millis(100));
        assert!(result.is_err(), "不存在的路径应返回错误");
    }

    #[test]
    fn daemon_stop_handle_is_shared() {
        let daemon = Daemon::new("/repo", "demo", 2000, "/tmp/db.lbug");
        let handle1 = daemon.stop_handle();
        let handle2 = daemon.stop_handle();
        assert!(!handle1.load(Ordering::SeqCst));
        handle1.store(true, Ordering::SeqCst);
        assert!(
            handle2.load(Ordering::SeqCst),
            "stop_handle 返回的 Arc 应共享状态"
        );
    }

    // --- LOG-005: daemon_event 事件发出验证 ---

    #[test]
    #[cfg(feature = "lang-c")]
    fn log_005_daemon_event_emitted_for_code_files() {
        let mut daemon = Daemon::new("/repo", "demo", 2000, "/tmp/db.lbug");
        let (observer, _call_count, _events) = CountingObserver::new();
        daemon.add_observer(Box::new(observer));

        let debounced_events = vec![
            make_event(
                EventKind::Create(notify_debouncer_full::notify::event::CreateKind::File),
                "main.rs",
            ),
            make_event(
                EventKind::Modify(notify_debouncer_full::notify::event::ModifyKind::Any),
                "lib.c",
            ),
        ];

        let captured = capture_tracing(|| {
            daemon.process_debounced_events(&debounced_events);
        });

        assert!(
            captured.contains("daemon_event"),
            "LOG-005: daemon_event 事件应被发出，实际捕获: {captured:?}"
        );
        let count = captured.matches("daemon_event").count();
        assert_eq!(
            count, 2,
            "LOG-005: 每个代码文件事件应发出一个 daemon_event，实际 {count}"
        );
        assert!(
            captured.contains("create") && captured.contains("modify"),
            "daemon_event 应携带 change_type 字段"
        );
        assert!(
            captured.contains("main.rs") && captured.contains("lib.c"),
            "daemon_event 应携带文件路径"
        );
    }

    #[test]
    fn log_005_daemon_event_not_emitted_for_non_code_files() {
        let mut daemon = Daemon::new("/repo", "demo", 2000, "/tmp/db.lbug");
        let (observer, _call_count, _events) = CountingObserver::new();
        daemon.add_observer(Box::new(observer));

        let debounced_events = vec![
            make_event(
                EventKind::Create(notify_debouncer_full::notify::event::CreateKind::File),
                "README.md",
            ),
            make_event(
                EventKind::Modify(notify_debouncer_full::notify::event::ModifyKind::Any),
                "config.ini",
            ),
        ];

        let captured = capture_tracing(|| {
            daemon.process_debounced_events(&debounced_events);
        });

        assert!(
            !captured.contains("daemon_event"),
            "LOG-005: 非代码文件不应触发 daemon_event 事件，实际捕获: {captured:?}"
        );
    }

    #[test]
    #[cfg(feature = "lang-python")]
    fn log_005_daemon_event_emitted_for_remove() {
        let mut daemon = Daemon::new("/repo", "demo", 2000, "/tmp/db.lbug");
        let (observer, _call_count, _events) = CountingObserver::new();
        daemon.add_observer(Box::new(observer));

        let debounced_events = vec![make_event(
            EventKind::Remove(notify_debouncer_full::notify::event::RemoveKind::File),
            "old.py",
        )];

        let captured = capture_tracing(|| {
            daemon.process_debounced_events(&debounced_events);
        });

        assert!(
            captured.contains("daemon_event"),
            "LOG-005: 删除事件应触发 daemon_event，实际捕获: {captured:?}"
        );
        assert!(
            captured.contains("remove"),
            "daemon_event 应携带 change_type=remove"
        );
    }
}
