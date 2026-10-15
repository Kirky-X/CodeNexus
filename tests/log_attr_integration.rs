// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! `#[forge::log]` (sdforge `inklog` feature) end-to-end anchor for the
//! CLI command entries carrying the attribute: `query` / `status` (full
//! success and failure paths) and `index` / `trace` / `impact` / `search` /
//! `context` (failure-path wiring).
//!
//! Both commands carry the declarative function-level logging attribute:
//! the macro wraps the original body in a shell that emits `fn_enter`
//! (debug, args masked) and `fn_exit` (info with `duration_ms` on success,
//! error on `Err`) through the `log` facade. This test dispatches through
//! the same sdforge inventory path `main.rs::run_cli` uses, which also pins
//! that stacking the attribute onto `#[forge(cli = true)]` keeps the CLI
//! handler registration intact and the handler's return semantics
//! unchanged.
//!
//! Observing `log` facade records requires a process-global capture logger;
//! this file is its own test binary, so `log::set_boxed_logger` is
//! unopposed here. The lib test process cannot host these assertions —
//! `logging::init_logging` already installs inklog's logger there.
//!
//! Scope note: these assertions anchor at the `log`-facade level (record
//! shape, levels, masking, return semantics). The production bridge —
//! `RUST_LOG` level filtering, per-target presets and the inklog file sink —
//! is exercised by the CLI-level runs; one-shot commands drain the async log
//! queue before exiting (`codenexus::logging::flush`).

#![cfg(feature = "cli")]

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use codenexus::kit::{build_kit, KitBootstrapConfig};
use codenexus::service::init_kit;
use sdforge::inventory;

/// Records emitted through the `log` facade, in emission order:
/// `(level, target, message)`.
static RECORDS: Mutex<Vec<(log::Level, String, String)>> = Mutex::new(Vec::new());

struct CapturingLogger;

impl log::Log for CapturingLogger {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        RECORDS.lock().expect("capture lock not poisoned").push((
            record.level(),
            record.target().to_string(),
            record.args().to_string(),
        ));
    }

    fn flush(&self) {}
}

/// Installs the capture logger once (the `log` facade is process-global) and
/// lifts the facade's max level so the macro's debug `fn_enter` gate opens.
fn install_capture_logger() {
    static INSTALL: OnceLock<()> = OnceLock::new();
    INSTALL.get_or_init(|| {
        log::set_boxed_logger(Box::new(CapturingLogger)).expect("capture logger installs once");
        log::set_max_level(log::LevelFilter::Trace);
    });
}

/// Records carrying `fn=<name>` — isolates one command's records from the
/// other's.
fn records_for(fn_name: &str) -> Vec<(log::Level, String, String)> {
    let needle = format!("fn={fn_name}");
    RECORDS
        .lock()
        .expect("capture lock not poisoned")
        .iter()
        .filter(|(_, _, message)| message.contains(&needle))
        .cloned()
        .collect()
}

/// CLI handler lookup — mirrors `main.rs::run_cli`'s inventory dispatch.
fn cli_handler(name: &str) -> &'static sdforge::cli::CliHandlerRegistration {
    inventory::iter::<sdforge::cli::CliHandlerRegistration>()
        .find(|handler| handler.name == name)
        .unwrap_or_else(|| panic!("`{name}` CLI handler must be registered"))
}

/// Dispatches one CLI command the way `run_cli` does. The handler serializes
/// the command's return value (`()` → `Value::Null` for these two).
fn dispatch(
    runtime: &tokio::runtime::Runtime,
    name: &str,
    args: HashMap<String, String>,
) -> Result<serde_json::Value, sdforge::prelude::ApiError> {
    runtime.block_on((cli_handler(name).handler)(args, None))
}

#[test]
fn cli_query_and_status_emit_forge_log_records() {
    // The failure and success phases deliberately share one #[test]:
    // `init_kit` installs a process-global Kit, and test order within a
    // binary is nondeterministic — a separate failure-path test would race
    // this test's initialization and assert against the wrong state.
    install_capture_logger();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");

    // --- failure path: no Kit initialized in this process -------------------
    // The credential-shaped tail proves the DataMasker pass: the macro
    // renders args lazily and masks them before the record reaches the
    // facade, so the plaintext must never appear in the captured output.
    let args = HashMap::from([(
        "cypher".to_string(),
        "MATCH (n) RETURN n LIMIT 1 // password=hunter2".to_string(),
    )]);
    assert!(
        dispatch(&runtime, "query", args).is_err(),
        "query without an initialized Kit must fail"
    );
    assert!(
        dispatch(&runtime, "status", HashMap::new()).is_err(),
        "status without an initialized Kit must fail"
    );

    // The five newer `#[forge::log]` adopters: same failure-path anchor.
    // Their success paths run through the identical macro machinery already
    // pinned by query/status above, so only wiring is asserted here — a
    // success-path `index` would drag the full indexing pipeline into this
    // test binary.
    let newer: Vec<(&str, HashMap<String, String>)> = vec![
        (
            "index",
            HashMap::from([
                ("path".to_string(), ".".to_string()),
                (
                    "name".to_string(),
                    "log-attr-test // password=hunter2".to_string(),
                ),
                ("force".to_string(), "false".to_string()),
                ("lsp".to_string(), "false".to_string()),
                ("embed".to_string(), "false".to_string()),
                ("ram_first".to_string(), "false".to_string()),
                ("fresh".to_string(), "false".to_string()),
            ]),
        ),
        (
            "trace",
            HashMap::from([
                (
                    "symbol".to_string(),
                    "no_such_symbol // password=hunter2".to_string(),
                ),
                ("trace_type".to_string(), "call".to_string()),
                ("depth".to_string(), "5".to_string()),
                ("path_filter".to_string(), String::new()),
                ("detect_cycles".to_string(), "false".to_string()),
                ("cross_service".to_string(), "false".to_string()),
            ]),
        ),
        (
            "impact",
            HashMap::from([
                (
                    "symbol".to_string(),
                    "no_such_symbol // password=hunter2".to_string(),
                ),
                ("depth".to_string(), "5".to_string()),
                ("edge_types".to_string(), "CALLS".to_string()),
                ("max_depth".to_string(), "5".to_string()),
                ("include_tests".to_string(), "false".to_string()),
            ]),
        ),
        (
            "search",
            HashMap::from([
                (
                    "text".to_string(),
                    "anything // password=hunter2".to_string(),
                ),
                ("fulltext".to_string(), "false".to_string()),
                ("limit".to_string(), "10".to_string()),
                ("mode".to_string(), "structured".to_string()),
                ("project".to_string(), String::new()),
            ]),
        ),
        (
            "context",
            HashMap::from([
                (
                    "symbol".to_string(),
                    "no_such_symbol // password=hunter2".to_string(),
                ),
                ("depth".to_string(), "3".to_string()),
                ("project".to_string(), String::new()),
                ("enhanced".to_string(), "false".to_string()),
            ]),
        ),
    ];
    for (name, args) in &newer {
        assert!(
            dispatch(&runtime, name, args.clone()).is_err(),
            "{name} without an initialized Kit must fail"
        );
    }
    for (name, _) in &newer {
        let records = records_for(name);
        let enter = records
            .iter()
            .find(|(level, _, message)| {
                *level == log::Level::Debug && message.contains(&format!("fn_enter fn={name}"))
            })
            .unwrap_or_else(|| panic!("{name} must emit a debug fn_enter record"));
        assert!(
            enter
                .2
                .contains(&format!("module=codenexus::service::{name}")),
            "{}",
            enter.2
        );
        let exit = records
            .iter()
            .find(|(level, _, message)| {
                *level == log::Level::Error && message.contains(&format!("fn_exit fn={name}"))
            })
            .unwrap_or_else(|| panic!("{name} must emit an error fn_exit record"));
        assert!(exit.2.contains("ok=false"), "{}", exit.2);
    }
    // Credential masking must hold for every args-carrying adopter: each
    // command above carries the canary in one of its string args, so a
    // masking regression fails here per command rather than only for
    // `query`.
    for name in ["index", "trace", "impact", "search", "context"] {
        let records = records_for(name);
        let enter = records
            .iter()
            .find(|(level, _, message)| *level == log::Level::Debug && message.contains("fn_enter"))
            .expect("fn_enter record");
        assert!(
            !enter.2.contains("hunter2"),
            "{name} args must be masked: {}",
            enter.2
        );
    }

    let query_records = records_for("query");
    let enter = query_records
        .iter()
        .find(|(level, _, message)| {
            *level == log::Level::Debug && message.contains("fn_enter fn=query")
        })
        .expect("debug fn_enter record");
    assert_eq!(
        enter.1, "sdforge::log_attr",
        "records originate in the macro's runtime support module; a `log` \
         filter directive has to name this target to tune them at the facade \
         level (the production pipeline re-emits through its logger bridge)"
    );
    assert!(
        enter.2.contains("module=codenexus::service::query"),
        "{}",
        enter.2
    );
    assert!(
        enter.2.contains("MATCH (n) RETURN n LIMIT 1"),
        "args payload must be captured: {}",
        enter.2
    );
    assert!(
        !enter.2.contains("hunter2"),
        "credential payload must be masked: {}",
        enter.2
    );
    assert!(
        enter.2.contains("[REDACTED]"),
        "masking replaces the credential value: {}",
        enter.2
    );

    let exit = query_records
        .iter()
        .find(|(level, _, message)| {
            *level == log::Level::Error && message.contains("fn_exit fn=query")
        })
        .expect("failure exit must log at error level");
    assert!(exit.2.contains("ok=false"), "{}", exit.2);
    assert!(exit.2.contains("duration_ms="), "{}", exit.2);
    assert!(
        exit.2.contains("error="),
        "err_detail must carry the error payload: {}",
        exit.2
    );

    let status_exit = records_for("status")
        .into_iter()
        .find(|(level, _, message)| {
            *level == log::Level::Error && message.contains("fn_exit fn=status")
        })
        .expect("failure exit must log at error level");
    assert!(status_exit.2.contains("ok=false"), "{}", status_exit.2);

    // --- success path: initialize the Kit the way the CLI does --------------
    let dir = tempfile::TempDir::new().expect("temp dir");
    let kit = runtime
        .block_on(build_kit(&KitBootstrapConfig::new(
            dir.path().join("log_attr.lbug"),
        )))
        .expect("build_kit");
    init_kit(kit).expect("init_kit");

    let args = HashMap::from([(
        "cypher".to_string(),
        "MATCH (n) RETURN n LIMIT 1".to_string(),
    )]);
    assert!(
        dispatch(&runtime, "query", args).is_ok(),
        "query with an initialized Kit must succeed"
    );
    let ok_exit = records_for("query")
        .into_iter()
        .find(|(level, _, message)| {
            *level == log::Level::Info && message.contains("fn_exit fn=query")
        })
        .expect("success exit must log at the configured level (info)");
    assert!(ok_exit.2.contains("ok=true"), "{}", ok_exit.2);
    assert!(ok_exit.2.contains("duration_ms="), "{}", ok_exit.2);
    assert!(
        !ok_exit.2.contains("result="),
        "the `result` flag is off, so no payload field may be emitted: {}",
        ok_exit.2
    );

    assert!(
        dispatch(&runtime, "status", HashMap::new()).is_ok(),
        "status with an initialized Kit must succeed"
    );
    let status_ok_exit = records_for("status")
        .into_iter()
        .find(|(level, _, message)| {
            *level == log::Level::Info && message.contains("fn_exit fn=status")
        })
        .expect("success exit must log at the configured level (info)");
    assert!(status_ok_exit.2.contains("ok=true"), "{}", status_ok_exit.2);
}
