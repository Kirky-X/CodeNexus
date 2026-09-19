// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Index observer: triggers incremental indexing on file changes.
use std::path::PathBuf;

use tracing::{info, warn};

use crate::daemon::event::{DaemonEvent, EventObserver};
use crate::index::{IndexError, IndexFacade, IndexResult};

/// 索引观察者：收到事件后触发增量索引。
///
/// 每次被通知时，调用 [`IndexFacade::index_incremental`] 对项目根目录
/// 执行增量索引。索引期间设置 `is_indexing` 标志。
pub struct IndexObserver {
    /// 索引门面。
    facade: IndexFacade,
    /// 项目名称。
    project_name: String,
    /// 监视的项目根目录。
    watch_path: PathBuf,
    /// 是否正在索引中。
    is_indexing: bool,
    /// 索引触发次数。
    index_count: usize,
    /// 最近一次成功的索引结果。
    last_result: Option<IndexResult>,
    /// 最近一次索引错误（索引失败时记录）。
    last_error: Option<IndexError>,
    /// 索引完成回调观察者（影响告警等），成功路径后按注册顺序回调。
    completion_observers: Vec<Box<dyn EventObserver + Send>>,
}

impl IndexObserver {
    /// 创建一个新的索引观察者。
    pub fn new(facade: IndexFacade, project_name: String, watch_path: PathBuf) -> Self {
        Self {
            facade,
            project_name,
            watch_path,
            is_indexing: false,
            index_count: 0,
            last_result: None,
            last_error: None,
            completion_observers: Vec::new(),
        }
    }

    /// 注册一个"索引完成"观察者（Observer 模式的完成回调侧）。
    ///
    /// 增量索引成功返回后，[`IndexObserver`] 会以
    /// `(&IndexResult, &[PathBuf])` 回调所有已注册的完成观察者。
    pub fn add_completion_observer(&mut self, observer: Box<dyn EventObserver + Send>) {
        self.completion_observers.push(observer);
    }

    /// 返回是否正在索引中。
    #[must_use]
    pub fn is_indexing(&self) -> bool {
        self.is_indexing
    }

    /// 返回索引触发次数。
    #[must_use]
    pub fn index_count(&self) -> usize {
        self.index_count
    }

    /// 返回最近一次成功的索引结果。
    #[must_use]
    pub fn last_result(&self) -> Option<&IndexResult> {
        self.last_result.as_ref()
    }

    /// 返回最近一次索引错误。
    #[must_use]
    pub fn last_error(&self) -> Option<&IndexError> {
        self.last_error.as_ref()
    }
}

impl EventObserver for IndexObserver {
    fn on_events(&mut self, events: &[DaemonEvent]) {
        //：索引期间暂停事件处理。
        self.is_indexing = true;
        self.index_count += 1;

        info!(
            project = %self.project_name,
            path = %self.watch_path.display(),
            "{}",
            crate::i18n::tr("index-incremental-triggered")
        );

        let changed_files: Vec<PathBuf> = events
            .iter()
            .map(|event| match event {
                DaemonEvent::Create(p) | DaemonEvent::Modify(p) | DaemonEvent::Remove(p) => {
                    p.clone()
                }
            })
            .collect();

        match self
            .facade
            .index_incremental(&self.watch_path, &self.project_name, false)
        {
            Ok(result) => {
                for observer in &mut self.completion_observers {
                    observer.on_index_complete(&result, &changed_files);
                }
                self.last_result = Some(result);
                self.last_error = None;
            }
            Err(err) => {
                warn!(
                    error = %err,
                    "{}",
                    crate::i18n::tr("index-incremental-failed")
                );
                self.last_error = Some(err);
            }
        }

        self.is_indexing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

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

    #[test]
    fn index_observer_new_initializes_fields() {
        let tmp = TempDir::new().unwrap();
        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let observer = IndexObserver::new(facade, "demo".to_string(), tmp.path().to_path_buf());
        assert!(!observer.is_indexing(), "初始状态不应在索引中");
        assert_eq!(observer.index_count(), 0, "初始索引次数应为 0");
        assert!(observer.last_result().is_none(), "初始无索引结果");
        assert!(observer.last_error().is_none(), "初始无错误");
    }

    #[test]
    fn index_observer_triggers_incremental_index() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "main.rs", "fn main() {}\n");

        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let mut observer = IndexObserver::new(facade, "demo".to_string(), tmp.path().to_path_buf());

        let events = vec![DaemonEvent::Modify(tmp.path().join("main.rs"))];
        observer.on_events(&events);

        assert_eq!(observer.index_count(), 1, "应触发一次索引");
        assert!(!observer.is_indexing(), "索引完成后应清除标志");
        assert!(observer.last_result().is_some(), "应有索引结果");
        assert!(observer.last_error().is_none(), "不应有错误");
    }

    #[test]
    fn index_observer_pauses_during_indexing() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");

        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let mut observer = IndexObserver::new(facade, "demo".to_string(), tmp.path().to_path_buf());

        assert!(!observer.is_indexing());

        let events = vec![DaemonEvent::Create(tmp.path().join("a.rs"))];
        observer.on_events(&events);

        assert!(
            !observer.is_indexing(),
            "BR-DAEMON-003：索引完成后应恢复事件处理"
        );
        assert_eq!(observer.index_count(), 1);
    }

    #[test]
    fn index_observer_records_error_on_failure() {
        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let mut observer = IndexObserver::new(
            facade,
            "demo".to_string(),
            PathBuf::from("/nonexistent/path/xyz"),
        );

        let events = vec![DaemonEvent::Modify(PathBuf::from(
            "/nonexistent/path/xyz/a.rs",
        ))];
        observer.on_events(&events);

        assert_eq!(observer.index_count(), 1, "仍应计数一次触发");
        assert!(!observer.is_indexing(), "失败后也应清除标志");
        assert!(observer.last_error().is_some(), "应记录错误");
        assert!(observer.last_result().is_none(), "失败时不应有结果");
    }

    #[test]
    fn index_observer_multiple_triggers_increment_count() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "a.rs", "fn a() {}\n");

        let db_path = fresh_db_path();
        let facade = IndexFacade::new(&db_path).expect("facade");
        let mut observer = IndexObserver::new(facade, "demo".to_string(), tmp.path().to_path_buf());

        for _ in 0..3 {
            observer.on_events(&[DaemonEvent::Modify(tmp.path().join("a.rs"))]);
        }
        assert_eq!(observer.index_count(), 3, "应触发三次索引");
    }
}
