// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Unified database-open entry point (crate-internal) — the single source of
//! truth for open semantics across all production call sites.
//!
//! Before this module, seven production sites opened [`Repository`] handles
//! directly (index pipeline, trace loader, service index/evolve, daemon
//! impact observer), each re-stating its own justification in comments. This
//! module owns those semantics once:
//!
//! - **Fresh MVCC snapshot** (read-only): every call opens a fresh snapshot,
//!   so the reader immediately sees other processes' committed writes — the
//!   kit's long-lived connection holds a boot-time snapshot and must NOT be
//!   reused for post-index reads.
//! - **Writable open with WAL replay**: read-only opens cannot replay WAL,
//!   so observers that must see the true durable state open writable.
//! - **Lock backoff**: both modes retry on open-time lock conflicts with
//!   exponential backoff before failing loud.

use std::path::Path;
use std::time::Duration;

use tracing::warn;

use super::error::{Result, StorageError};
use super::repository::Repository;

/// Open mode for [`open_fresh`] — the tri-state semantics documented at the
/// module level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    /// Fresh MVCC snapshot, read-only. Post-index reads (index DQ re-check,
    /// evolve architecture metrics) MUST use this: a long-lived kit connection
    /// sees stale data.
    ReadOnly,
    /// Writable open with WAL replay. Observers that must observe the durable
    /// state (daemon impact observer) MUST use this: read-only opens cannot
    /// replay WAL.
    ReadWrite,
}

/// Opens a fresh [`Repository`] handle with lock backoff — the crate's only
/// sanctioned production open path.
///
/// # Errors
///
/// - [`StorageError::DatabaseLocked`] when another holder keeps the file lock
///   beyond the backoff budget (fail loud, classified — not a generic error).
/// - [`StorageError::Database`] / [`StorageError::Io`] for engine-level open
///   failures (propagated immediately, no retry).
pub(crate) fn open_fresh(db_path: impl AsRef<Path>, mode: OpenMode) -> Result<Repository> {
    let path = db_path.as_ref();
    retry_on_lock(
        3,
        is_lock_conflict,
        || StorageError::DatabaseLocked {
            holder_hint: format!("open_fresh({})", path.display()),
        },
        || match mode {
            OpenMode::ReadOnly => Repository::open_read_only(path),
            OpenMode::ReadWrite => Repository::open(path),
        },
    )
}

/// Lock-conflict classification for any [`StorageError`] — the single
/// predicate the crate uses to decide "transient lock, worth retrying".
///
/// Covers:
/// - open-time hard locks: [`StorageError::DatabaseLocked`] (the connection
///   layer's classification of engine open failures), plus raw engine errors
///   whose display carries the open-time lock strings;
/// - query-time transient conflicts: [`StorageError::Query`] messages matching
///   the shape set that used to live in `index::pipeline::is_lock_conflict`
///   (`"locked"` / `"Lock"` / the lbug 0.18.2 in-process single-writer phrase).
pub(crate) fn is_lock_conflict(e: &StorageError) -> bool {
    match e {
        StorageError::DatabaseLocked { .. } => true,
        StorageError::Database(inner) => {
            super::connection::is_db_locked(e) || is_query_lock_text(&inner.to_string())
        }
        StorageError::Query(msg) => is_query_lock_text(msg),
        _ => false,
    }
}

/// Query-time lock message shapes (verbatim from the classifier that used to
/// live in `index::pipeline::is_lock_conflict` — behavior preservation, not
/// taste).
fn is_query_lock_text(msg: &str) -> bool {
    msg.contains("locked")
        || msg.contains("Lock")
        || msg.contains("Only one write transaction at a time")
}

/// Exponential-backoff retry for transient lock conflicts (synchronous
/// callers only).
///
/// Backoff: 100ms start, ×2.0 factor, 400ms cap, no jitter (matches the
/// budget the index pipeline used before this moved here). A failure
/// classified by `is_lock` is retried up to `max_retries` times (so
/// `max_retries + 1` total attempts); any other error propagates
/// immediately. When all attempts are exhausted on a lock error, `to_locked`
/// produces the caller's classified error.
///
/// # Blocking note (async callers must not use this)
///
/// Retries sleep with [`std::thread::sleep`] — blocking. Synchronous call
/// sites only; async contexts must not call this on the runtime thread (wrap
/// in `spawn_blocking` or use an async sleep instead).
pub(crate) fn retry_on_lock<T, E>(
    max_retries: u32,
    is_lock: impl Fn(&E) -> bool,
    to_locked: impl Fn() -> E,
    mut f: impl FnMut() -> std::result::Result<T, E>,
) -> std::result::Result<T, E> {
    for attempt in 0..=max_retries {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) if is_lock(&e) => {
                if attempt < max_retries {
                    let delay = limiteron::retry::delay_for_attempt(
                        attempt + 1,
                        Duration::from_millis(100),
                        2.0,
                        Duration::from_millis(400),
                        0.0,
                    );
                    warn!(
                        attempt = attempt + 1,
                        max_retries,
                        delay_ms = delay.as_millis(),
                        "database locked, retrying"
                    );
                    std::thread::sleep(delay);
                    continue;
                }
                return Err(to_locked());
            }
            Err(e) => return Err(e),
        }
    }
    Err(to_locked())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::node::Node;
    use crate::model::node_label::NodeLabel;

    fn db_locked(hint: &str) -> StorageError {
        StorageError::DatabaseLocked {
            holder_hint: hint.into(),
        }
    }

    fn tmp_db(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("open_fresh_test.lbug")
    }

    fn sample_project(id: &str, name: &str) -> Node {
        Node::builder(NodeLabel::Project, name, name)
            .id(id)
            .project(name)
            .build()
    }

    /// 三态语义·读写：ReadWrite 模式可写可提交。
    #[test]
    fn readwrite_mode_accepts_writes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = tmp_db(dir.path());
        let repo = open_fresh(&db, OpenMode::ReadWrite).expect("rw open");
        repo.save_project(&sample_project("p1", "demo"))
            .expect("write");
    }

    /// 三态语义·只读 fresh：另一句柄提交写入后，新 ReadOnly 句柄立即可见
    /// （写后立即可读——长连接陈旧 MVCC 快照问题的行为反证）。
    #[test]
    fn fresh_readonly_sees_committed_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = tmp_db(dir.path());
        {
            let writer = open_fresh(&db, OpenMode::ReadWrite).expect("rw open");
            writer
                .save_project(&sample_project("p1", "demo"))
                .expect("write");
        }
        let reader = open_fresh(&db, OpenMode::ReadOnly).expect("ro open");
        let projects = reader.list_projects().expect("read");
        assert!(
            projects.iter().any(|p| p.id == "p1"),
            "fresh read-only open must see the committed write"
        );
    }

    /// 锁退避·成功路径：前两次锁冲突被退避吸收，第三次成功。
    #[test]
    fn retry_on_lock_succeeds_after_transient_locks() {
        let mut attempts = 0;
        let result: Result<u8> = retry_on_lock(
            3,
            |e: &StorageError| matches!(e, StorageError::DatabaseLocked { .. }),
            || db_locked(""),
            || {
                attempts += 1;
                if attempts < 3 {
                    Err(db_locked(""))
                } else {
                    Ok(attempts)
                }
            },
        );
        assert_eq!(result.expect("retry succeeds"), 3);
        assert_eq!(attempts, 3);
    }

    /// 锁退避·耗尽路径：持续锁冲突在预算耗尽后收敛为分类错误。
    #[test]
    fn retry_on_lock_exhausts_into_classified_error() {
        let result: Result<()> = retry_on_lock(
            2,
            |e: &StorageError| matches!(e, StorageError::DatabaseLocked { .. }),
            || db_locked("test"),
            || Err(db_locked("")),
        );
        match result {
            Err(StorageError::DatabaseLocked { holder_hint }) => {
                assert_eq!(holder_hint, "test");
            }
            other => panic!("expected classified DatabaseLocked, got {other:?}"),
        }
    }

    /// 非锁错误立即传播，不重试。
    #[test]
    fn retry_on_lock_propagates_unrelated_errors() {
        let mut attempts = 0;
        let result: Result<()> = retry_on_lock(
            3,
            |e: &StorageError| matches!(e, StorageError::DatabaseLocked { .. }),
            || db_locked(""),
            || {
                attempts += 1;
                Err(StorageError::Io(std::io::Error::other("disk gone")))
            },
        );
        assert!(matches!(result, Err(StorageError::Io(_))));
        assert_eq!(attempts, 1, "non-lock errors must not be retried");
    }

    // --- 时序敏感回归（自 index::pipeline::with_retry 迁入） ---

    /// Backoff sequence must match `limiteron::retry::delay_for_attempt`
    /// exactly (100/200/400ms for retries 1..=3), then surface the classified
    /// error. Observed durations are measured between successive `f`
    /// invocations; each must be at least the expected delay (sleep never
    /// returns early) and within scheduling slack of it.
    #[test]
    fn retry_on_lock_backoff_matches_limiteron_delay_for_attempt() {
        use std::time::Instant;

        let mut timestamps = Vec::new();
        let result: Result<u32> = retry_on_lock(
            3,
            |e: &StorageError| matches!(e, StorageError::DatabaseLocked { .. }),
            || db_locked("exhausted"),
            || {
                timestamps.push(Instant::now());
                Err(db_locked(""))
            },
        );
        assert!(
            matches!(result, Err(StorageError::DatabaseLocked { .. })),
            "lock errors exhausting retries must surface DatabaseLocked, got: {result:?}"
        );
        assert_eq!(
            timestamps.len(),
            4,
            "max_retries=3 means 1 initial + 3 retries"
        );
        for retry in 1..=3u32 {
            let expected = limiteron::retry::delay_for_attempt(
                retry,
                Duration::from_millis(100),
                2.0,
                Duration::from_millis(400),
                0.0,
            );
            let observed = timestamps[retry as usize] - timestamps[retry as usize - 1];
            assert!(
                observed >= expected,
                "retry {retry}: backoff {observed:?} must be at least delay_for_attempt's {expected:?}"
            );
            let slack = Duration::from_millis(150);
            assert!(
                observed <= expected + slack,
                "retry {retry}: backoff {observed:?} must track delay_for_attempt's {expected:?} \
                 (linux sleep can overshoot, but not by more than {slack:?} slack)"
            );
        }
    }

    /// The 400ms cap: retry 4 must NOT double to 800ms (the hand-rolled
    /// `delay_ms *= 2` behavior) — `delay_for_attempt` caps at `max_delay`.
    #[test]
    fn retry_on_lock_backoff_caps_at_400ms() {
        use std::time::Instant;

        let mut timestamps = Vec::new();
        let result: Result<u32> = retry_on_lock(
            4,
            |e: &StorageError| matches!(e, StorageError::DatabaseLocked { .. }),
            || db_locked("exhausted"),
            || {
                timestamps.push(Instant::now());
                Err(db_locked(""))
            },
        );
        assert!(matches!(result, Err(StorageError::DatabaseLocked { .. })));
        assert_eq!(timestamps.len(), 5, "max_retries=4 means 5 total attempts");
        // Retries 1..=4 all equal the capped 400ms delay, never 800ms.
        for retry in 1..=4u32 {
            let expected = limiteron::retry::delay_for_attempt(
                retry,
                Duration::from_millis(100),
                2.0,
                Duration::from_millis(400),
                0.0,
            );
            let observed = timestamps[retry as usize] - timestamps[retry as usize - 1];
            assert!(
                observed >= expected,
                "retry {retry}: backoff {observed:?} must be at least delay_for_attempt's {expected:?}"
            );
            let slack = Duration::from_millis(150);
            assert!(
                observed <= expected + slack,
                "retry {retry}: backoff {observed:?} must equal the capped {expected:?} \
                 (uncapped hand-rolled doubling would sleep 800ms here)"
            );
        }
    }

    /// `max_retries = 0` performs exactly one attempt and never sleeps.
    #[test]
    fn retry_on_lock_zero_max_retries_does_not_wait() {
        use std::time::Instant;

        let mut attempts = 0;
        let start = Instant::now();
        let result: Result<()> = retry_on_lock(
            0,
            |e: &StorageError| matches!(e, StorageError::DatabaseLocked { .. }),
            || db_locked("exhausted"),
            || {
                attempts += 1;
                Err(db_locked(""))
            },
        );
        let elapsed = start.elapsed();
        assert!(
            matches!(result, Err(StorageError::DatabaseLocked { .. })),
            "should return DatabaseLocked with 0 retries: {result:?}"
        );
        assert_eq!(attempts, 1);
        assert!(
            elapsed < Duration::from_millis(50),
            "max_retries=0 must not sleep (first backoff is 100ms), took {elapsed:?}"
        );
    }
}
