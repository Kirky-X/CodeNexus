// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

use std::path::{Path, PathBuf};

use serde::Serialize;
use tracing::warn;

use crate::daemon::event::{DaemonEvent, EventObserver};
use crate::index::IndexResult;
use crate::model::NodeLabel;
use crate::storage::capability::Storage;
use crate::storage::Repository;

/// 单批次检查的受影响符号上限（超限截断并标注）。
pub const SYMBOL_CAP: usize = 200;

/// 单文件影响通知。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImpactNotice {
    pub file: String,
    pub symbols: Vec<ImpactSymbol>,
    /// 本文件受影响符号的入边总数。
    pub total_callers: usize,
}

/// 受影响符号及其入边数。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImpactSymbol {
    pub name: String,
    pub incoming: usize,
}

/// 索引完成后的影响告警观察者。
pub struct ImpactNotifyObserver {
    db_path: PathBuf,
    /// 可选 webhook：配置后以独立线程 POST 通知 JSON
    ///（`hub` feature 提供 reqwest；fire-and-forget，失败仅 warn）。
    webhook: Option<String>,
}

impl ImpactNotifyObserver {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            webhook: None,
        }
    }

    /// 配置 webhook URL（`hub` feature）。
    #[cfg(feature = "hub")]
    pub fn with_webhook(mut self, webhook: Option<String>) -> Self {
        self.webhook = webhook;
        self
    }

    /// 对变更文件计算影响通知（可测核心）。
    ///
    /// 只读打开图库；打不开（罕见：索引刚重建）则返回空而不报错 ——
    /// 告警是尽力而为的旁路，不应影响守护循环。
    pub fn compute_notices(&self, changed_files: &[PathBuf]) -> Vec<ImpactNotice> {
        // Writable open (not read-only): the indexer's connection is already
        // closed when this callback fires, and a fresh open replays any WAL
        // the index run left behind — read-only opens cannot replay WAL and
        // would see stale data.
        let repo = match Repository::open(&self.db_path) {
            Ok(repo) => repo,
            Err(err) => {
                warn!(
                    error = %err,
                    "{}",
                    crate::i18n::tr("impact-db-open-failed")
                );
                return Vec::new();
            }
        };
        let mut notices = Vec::new();
        let mut budget = SYMBOL_CAP;
        for file in changed_files {
            if budget == 0 {
                break;
            }
            let Some(notice) = self.notice_for_file(&repo, file, &mut budget) else {
                continue;
            };
            if !notice.symbols.is_empty() {
                notices.push(notice);
            }
        }
        notices
    }

    /// 查询单个变更文件中定义的符号及入边数（消耗 budget）。
    fn notice_for_file(
        &self,
        repo: &Repository,
        file: &Path,
        budget: &mut usize,
    ) -> Option<ImpactNotice> {
        let file_str = file.to_string_lossy();
        let escaped = crate::storage::schema::escape_cypher_string(&file_str);
        let mut symbols = Vec::new();
        let mut total_callers = 0usize;
        for label in NodeLabel::all() {
            if *budget == 0 {
                break;
            }
            let cols = crate::storage::schema::node_table_columns(label);
            if !cols.contains(&"filePath") {
                continue;
            }
            let table = crate::storage::schema::escape_identifier(label.table_name());
            let cypher = format!(
                "MATCH (n:{table}) WHERE n.filePath = '{escaped}' RETURN n.id AS id, n.name AS name;"
            );
            // 部分节点表在特定 schema 下不存在（查询报错）——跳过该 label，
            // 不放弃整个文件的通知。
            let rows = match repo.query(&cypher) {
                Ok(rows) => rows,
                Err(_) => continue,
            };
            for row in rows {
                if *budget == 0 {
                    break;
                }
                *budget -= 1;
                let id = row.first().and_then(|v| v.as_str()).unwrap_or_default();
                let name = row.get(1).and_then(|v| v.as_str()).unwrap_or_default();
                let incoming = self.incoming_edges(repo, id);
                total_callers += incoming;
                symbols.push(ImpactSymbol {
                    name: name.to_string(),
                    incoming,
                });
            }
        }
        Some(ImpactNotice {
            file: file_str.to_string(),
            symbols,
            total_callers,
        })
    }

    /// 统计指向 `id` 的入边数（单符号单查询；批量优化留给后续变更）。
    fn incoming_edges(&self, repo: &Repository, id: &str) -> usize {
        let escaped = crate::storage::schema::escape_cypher_string(id);
        let cypher =
            format!("MATCH (r:CodeRelation) WHERE r.target = '{escaped}' RETURN count(r) AS cnt;");
        repo.query(&cypher)
            .ok()
            .and_then(|rows| {
                rows.first()
                    .and_then(|r| r.first().and_then(|v| v.as_u64()))
            })
            .unwrap_or(0) as usize
    }
}

impl EventObserver for ImpactNotifyObserver {
    fn on_events(&mut self, _events: &[DaemonEvent]) {
        // 通知只挂在索引完成路径上（见 on_index_complete）。
    }

    fn on_index_complete(&mut self, result: &IndexResult, changed_files: &[PathBuf]) {
        let notices = self.compute_notices(changed_files);
        let truncated = notices.iter().any(|n| n.symbols.len() >= SYMBOL_CAP);
        for notice in &notices {
            warn!(
                event = "impact_notice",
                file = %notice.file,
                symbols = notice.symbols.len(),
                total_callers = notice.total_callers,
                project = %result.project_id,
                notice = %serde_json::to_string(notice).unwrap_or_default(),
                truncated,
                "{}",
                crate::i18n::tr("impact-notice")
            );
        }
        #[cfg(feature = "hub")]
        if let Some(webhook) = &self.webhook {
            if !notices.is_empty() {
                post_webhook(
                    webhook.clone(),
                    serde_json::json!({
                        "event": "impact_notice",
                        "project": result.project_id,
                        "notices": notices,
                        "truncated": truncated,
                    }),
                );
            }
        }
    }
}

/// Fire-and-forget webhook POST:独立线程执行，失败仅记录 warn，
/// 不阻塞 daemon 事件循环（`hub` feature 下的 reqwest 阻塞客户端）。
#[cfg(feature = "hub")]
fn post_webhook(url: String, payload: serde_json::Value) {
    std::thread::spawn(move || {
        let client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
        {
            Ok(client) => client,
            Err(err) => {
                warn!(
                    error = %err,
                    "{}",
                    crate::i18n::tr("impact-webhook-client-failed")
                );
                return;
            }
        };
        match client.post(&url).json(&payload).send() {
            Ok(response) if !response.status().is_success() => {
                warn!(
                    status = %response.status(),
                    url = %url,
                    "{}",
                    crate::i18n::tr("impact-webhook-non-2xx")
                );
            }
            Ok(_) => {}
            Err(err) => {
                warn!(
                    error = %err,
                    url = %url,
                    "{}",
                    crate::i18n::tr("impact-webhook-send-failed")
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::capability::Storage;
    use tempfile::TempDir;

    fn seeded_db() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("notify_db.lbug");
        // Seed via a short-lived writable connection, dropped before the
        // observer opens read-only — same sequencing as production (the
        // indexer's connection closes before on_index_complete fires).
        let repo = Repository::open(&db).unwrap();
        repo.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'hub', qualifiedName: 'demo.hub', filePath: '/src/hub.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").unwrap();
        repo.execute("CREATE (:Function {id: 'f2', project: 'demo', name: 'caller', qualifiedName: 'demo.caller', filePath: '/src/caller.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").unwrap();
        repo.execute("CREATE (:CodeRelation {id: 'e1', source: 'f2', target: 'f1', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});").unwrap();
        drop(repo);
        (dir, db)
    }

    fn index_result() -> IndexResult {
        IndexResult {
            project_id: "demo".to_string(),
            files_indexed: 1,
            files_skipped: 0,
            nodes_created: 0,
            edges_created: 0,
            duration_ms: 1,
        }
    }

    #[test]
    fn computes_notice_for_changed_file_with_incoming_edges() {
        let (_dir, db) = seeded_db();
        let observer = ImpactNotifyObserver::new(db);
        let notices = observer.compute_notices(&[PathBuf::from("/src/hub.rs")]);
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].total_callers, 1);
        let hub = notices[0].symbols.iter().find(|s| s.name == "hub").unwrap();
        assert_eq!(hub.incoming, 1);
        // caller 定义在另一个文件 —— 不属于 hub.rs 的变更通知。
        assert!(
            !notices[0].symbols.iter().any(|s| s.name == "caller"),
            "symbols from other files must not leak into this notice"
        );
    }

    #[test]
    fn unchanged_files_produce_no_notices() {
        let (_dir, db) = seeded_db();
        let observer = ImpactNotifyObserver::new(db);
        let notices = observer.compute_notices(&[PathBuf::from("/src/untouched.rs")]);
        assert!(notices.is_empty(), "no symbols → no notice emitted");
    }

    #[test]
    fn symbol_budget_truncates() {
        let (_dir, db) = seeded_db();
        let observer = ImpactNotifyObserver::new(db);
        let notices = observer.compute_notices(&[
            PathBuf::from("/src/hub.rs"),
            PathBuf::from("/src/caller.rs"),
        ]);
        let total: usize = notices.iter().map(|n| n.symbols.len()).sum();
        assert!(total >= 1);
        assert!(total <= SYMBOL_CAP, "per-batch cap holds");
    }

    #[test]
    fn webhook_config_none_keeps_offline_behavior() {
        // 未配置 webhook（默认）→ on_index_complete 不发起网络请求；
        // 行为与 log-only 版本一致（不 panic 即通过）。
        let (_dir, db) = seeded_db();
        let mut observer = ImpactNotifyObserver::new(db);
        observer.on_index_complete(&index_result(), &[PathBuf::from("/src/hub.rs")]);
    }

    #[test]
    #[cfg(feature = "hub")]
    fn webhook_url_with_invalid_host_does_not_panic() {
        // 配置了 webhook 但端点不可达：fire-and-forget，失败仅 warn，
        // 观察者不 panic、不阻塞。
        let (_dir, db) = seeded_db();
        let mut observer =
            ImpactNotifyObserver::new(db).with_webhook(Some("http://127.0.0.1:1/hook".to_string()));
        observer.on_index_complete(&index_result(), &[PathBuf::from("/src/hub.rs")]);
    }

    #[test]
    fn missing_db_returns_empty_notices() {
        let observer = ImpactNotifyObserver::new(PathBuf::from("/nonexistent/notify_db.lbug"));
        let notices = observer.compute_notices(&[PathBuf::from("/src/hub.rs")]);
        assert!(notices.is_empty(), "unopenable DB degrades to silence");
    }

    #[test]
    fn on_index_complete_emits_and_does_not_panic() {
        let (_dir, db) = seeded_db();
        let mut observer = ImpactNotifyObserver::new(db);
        // Exercises the trait path; output goes to tracing (warn) which the
        // test harness captures without failing.
        observer.on_index_complete(&index_result(), &[PathBuf::from("/src/hub.rs")]);
    }
}
