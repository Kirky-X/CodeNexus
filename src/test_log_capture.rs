// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Test log capture utilities backed by inklog's `LoggerSubscriber`.
//!
//! Provides [`capture_tracing`] and [`capture_tracing_debug`] for unit tests
//! that need to assert on tracing event output. Each function installs a
//! scoped `tracing_subscriber::registry` with an inklog `LoggerSubscriber`
//! layer, runs the supplied closure, then drains the console channel and
//! formats each `LogRecord` into a single string for `.contains` assertions.
//!
//! # Format
//!
//! Each captured event is rendered as `"{message} {key}={value} ..."` (one
//! line per event). Structured fields appear as `key=value` pairs after the
//! message, preserving compatibility with existing `.contains` assertions.

use std::sync::Arc;

use crossbeam_channel::unbounded;
use inklog::domain::core::LoggerSubscriber;
use inklog::LogRecord;
use inklog::Metrics;
use tracing_subscriber::prelude::*;

/// Formats a `LogRecord` into a string containing the message and all
/// structured fields as `key=value` pairs.
fn format_record(record: &LogRecord) -> String {
    let mut output = record.message.clone();
    for (key, value) in &record.fields {
        output.push(' ');
        output.push_str(key);
        output.push('=');
        output.push_str(&value.to_string());
    }
    output
}

/// Drains all pending records from `rx` and concatenates them into a single
/// string (one line per record).
///
/// `LoggerSubscriber::on_event` calls `try_send` synchronously, so by the time
/// `with_default(f)` returns, all events are already in the channel — `try_recv`
/// drains them without blocking.
pub fn drain_to_string(rx: &crossbeam_channel::Receiver<Arc<LogRecord>>) -> String {
    let mut output = String::new();
    while let Ok(record) = rx.try_recv() {
        output.push_str(&format_record(&record));
        output.push('\n');
    }
    output
}

/// Runs `f` inside a scoped tracing subscriber (all levels) that captures
/// all event output into a string, returning that string.
///
/// The throwaway `Dispatch::new` inside the window forces tracing to rebuild
/// its process-wide callsite-interest cache while THIS capture is the active
/// dispatcher (see `capture_tracing_debug` for why that cache must be
/// re-anchored per capture).
pub fn capture_tracing<R>(f: impl FnOnce() -> R) -> String {
    let (console_tx, console_rx) = unbounded::<Arc<LogRecord>>();
    let (async_tx, _async_rx) = unbounded::<Arc<LogRecord>>();
    let metrics = Arc::new(Metrics::new());
    let layer = LoggerSubscriber::new(console_tx, async_tx, metrics);
    let registry = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(registry, || {
        let _rebuild = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::new());
        f()
    });
    drain_to_string(&console_rx)
}

/// Runs `f` inside a scoped tracing subscriber that captures all event output
/// (all levels) into a string, returning that string.
///
/// Historically this installed a `LevelFilter::DEBUG` per-layer filter. That
/// filter is the root cause of the LOG-003 flake and must not come back:
/// a per-layer filter makes the registry compute a callsite's cached
/// `Interest` through a **thread-local** `FilterState`, which concurrent
/// tests' rebuilds can poison to `Interest::never` (a below-filter callsite
/// deposits a `never` that the next rebuild takes for a different callsite).
/// Once a callsite is cached `never`, its events are skipped process-wide —
/// the emitting thread never reaches the active subscriber — so a capture
/// running on another thread silently loses the event it asserts on.
/// Without a per-layer filter the registry answers `Interest::always()`
/// directly and the interest can never collapse to `never` while this
/// subscriber is alive. Callers assert with `.contains`, so the extra
/// captured levels are harmless.
///
/// Version note: the rebuild trick relies on `Interest::and` NOT being
/// never-dominant (tracing-core 0.1.36: differing interests fold to
/// `sometimes`, so our `always` keeps every callsite dispatchable). If a
/// tracing upgrade ever makes captures come back empty across the board,
/// check `Interest::and`'s semantics first.
pub fn capture_tracing_debug<R>(f: impl FnOnce() -> R) -> String {
    capture_tracing(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_tracing_captures_info_event() {
        let captured = capture_tracing(|| {
            tracing::info!("test_marker_001");
        });
        assert!(
            captured.contains("test_marker_001"),
            "expected captured output to contain the event message, got: {captured:?}"
        );
    }

    #[test]
    fn capture_tracing_captures_structured_fields() {
        let captured = capture_tracing(|| {
            tracing::info!(
                event = "file_parsed",
                path = "a.rs",
                nodes = 1usize,
                "file parsed"
            );
        });
        assert!(
            captured.contains("file_parsed"),
            "missing event field value: {captured:?}"
        );
        assert!(
            captured.contains("a.rs"),
            "missing path value: {captured:?}"
        );
        assert!(
            captured.contains("nodes"),
            "missing nodes field name: {captured:?}"
        );
    }

    #[test]
    fn capture_tracing_debug_captures_debug_and_above() {
        let captured = capture_tracing_debug(|| {
            tracing::debug!(event = "debug_evt", "debug msg");
            tracing::info!(event = "info_evt", "info msg");
        });
        assert!(
            captured.contains("debug_evt"),
            "DEBUG event should be captured: {captured:?}"
        );
        assert!(
            captured.contains("info_evt"),
            "INFO event should be captured: {captured:?}"
        );
    }

    #[test]
    fn capture_tracing_captures_multiple_events() {
        let captured = capture_tracing(|| {
            tracing::info!("first_evt");
            tracing::info!("second_evt");
        });
        assert!(
            captured.contains("first_evt"),
            "first event missing: {captured:?}"
        );
        assert!(
            captured.contains("second_evt"),
            "second event missing: {captured:?}"
        );
    }
}
