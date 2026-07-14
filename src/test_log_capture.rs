// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Test log capture utilities backed by inklog's `LoggerSubscriber`.
//!
//! Provides [`capture_tracing`] and [`capture_tracing_debug`] for unit tests
//! that need to assert on tracing event output. Each function installs a
//! scoped `tracing_subscriber::registry()` with an inklog `LoggerSubscriber`
//! layer, runs the supplied closure, then drains the console channel and
//! formats each `LogRecord` into a single string for `.contains()` assertions.
//!
//! # Format
//!
//! Each captured event is rendered as `"{message} {key}={value} ..."` (one
//! line per event). Structured fields appear as `key=value` pairs after the
//! message, preserving compatibility with existing `.contains()` assertions.

use std::sync::Arc;

use crossbeam_channel::unbounded;
use inklog::domain::core::LoggerSubscriber;
use inklog::LogRecord;
use inklog::Metrics;
use tracing_subscriber::filter::LevelFilter;
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
pub fn capture_tracing<R>(f: impl FnOnce() -> R) -> String {
    let (console_tx, console_rx) = unbounded::<Arc<LogRecord>>();
    let (async_tx, _async_rx) = unbounded::<Arc<LogRecord>>();
    let metrics = Arc::new(Metrics::new());
    let layer = LoggerSubscriber::new(console_tx, async_tx, metrics);
    let registry = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(registry, f);
    drain_to_string(&console_rx)
}

/// Runs `f` inside a scoped tracing subscriber (DEBUG and above) that
/// captures all event output into a string, returning that string.
pub fn capture_tracing_debug<R>(f: impl FnOnce() -> R) -> String {
    let (console_tx, console_rx) = unbounded::<Arc<LogRecord>>();
    let (async_tx, _async_rx) = unbounded::<Arc<LogRecord>>();
    let metrics = Arc::new(Metrics::new());
    let layer =
        LoggerSubscriber::new(console_tx, async_tx, metrics).with_filter(LevelFilter::DEBUG);
    let registry = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(registry, f);
    drain_to_string(&console_rx)
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
