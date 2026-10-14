// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! inklog-backed global logging: initialization, per-target level presets and
//! runtime level hot-tuning.
//!
//! Configuration goes through [`InklogConfig`] rather than inklog's fluent
//! builder because the per-target preset map (`InklogConfig::target_levels`)
//! has no builder setter; every builder knob used here maps 1:1 onto a config
//! field (see inklog `domain/core/builder.rs`).
//!
//! `RUST_LOG` is not forwarded by hand: inklog appends it to the `EnvFilter`
//! after the global level and the presets, so directive lists such as
//! `codenexus=debug,sqlx=warn` reach the filter with `RUST_LOG` winning over
//! presets, which in turn win over the global level (inklog
//! `domain/core/manager.rs`). CodeNexus only has to keep the *configured*
//! global level a plain level name — a directive list handed to the builder's
//! `level()` fails its level validation and aborts the build.
//!
//! Installed levels stay re-tunable at runtime through [`set_level`]: inklog
//! rebuilds the `EnvFilter` and hot-swaps it through
//! `tracing_subscriber::reload`, no restart required. Because `RUST_LOG`
//! keeps the highest priority across rebuilds, a bare level in it
//! (`RUST_LOG=info`) out-ranks a global hot-tune — unset `RUST_LOG` for the
//! tune to take effect; per-target entries are unaffected.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use inklog::{ConsoleSinkConfig, FileSinkConfig, InklogConfig, LogLevel, LoggerManager};

/// Global level used when `RUST_LOG` carries no plain level name.
const DEFAULT_LEVEL: &str = "info";

/// Per-target level presets: `target=level` pairs separated by commas.
const TARGET_LEVELS_ENV: &str = "CODENEXUS_LOG_TARGETS";

/// Log file inside [`crate::DEFAULT_DB_DIR`] so a one-off command in any
/// project creates only the side-effect directory users already gitignore.
/// inklog's file sink creates missing parent directories itself.
fn log_file() -> PathBuf {
    PathBuf::from(crate::DEFAULT_DB_DIR).join("logs/codenexus.log")
}

const LOG_FORMAT: &str = "{timestamp} [{level}] {target} - {message}";

/// Every console level is routed to stderr so stdout stays a pure JSON
/// channel under `> result.json` redirection. inklog's default is
/// `["error", "warn"]` (info goes to stdout), which corrupted
/// `codenexus index > result.json` whenever the pipeline emitted info! events.
/// Unknown level names in the list never match and are harmless.
const CONSOLE_STDERR_LEVELS: &[&str] = &["error", "warn", "info", "debug", "trace", "fatal"];

const LOG_CHANNEL_CAPACITY: usize = 10_000;

const LOG_FILE_MAX_SIZE: &str = "100MB";

const LOG_FILE_KEEP_FILES: u32 = 30;

/// The installed logger. A `static` is never dropped, so its sink workers
/// stay alive for the whole process — the effect `std::mem::forget` had
/// before this module retained the manager for [`set_level`].
static LOGGER: OnceLock<LoggerManager> = OnceLock::new();

/// Canonical `tracing` level name for `s`, accepting inklog's level aliases
/// (`warning` → `warn`, `critical`/`fatal` → `error`). `EnvFilter` only
/// understands the canonical five, so alias normalization happens here.
fn canonical_level(s: &str) -> Option<&'static str> {
    match LogLevel::from_str(s)? {
        LogLevel::Trace => Some("trace"),
        LogLevel::Debug => Some("debug"),
        LogLevel::Info => Some("info"),
        LogLevel::Warn => Some("warn"),
        LogLevel::Error | LogLevel::Fatal => Some("error"),
    }
}

/// Global level implied by a `RUST_LOG` value: its last plain level name
/// (`RUST_LOG=debug`), or [`DEFAULT_LEVEL`] when the value only holds
/// per-target directives (`RUST_LOG=codenexus=debug,sqlx=warn`) or garbage.
fn global_level(rust_log: &str) -> &'static str {
    rust_log
        .split(',')
        .filter(|token| !token.contains('='))
        .filter_map(|token| canonical_level(token.trim()))
        .next_back()
        .unwrap_or(DEFAULT_LEVEL)
}

/// Parses [`TARGET_LEVELS_ENV`] (`hyper=warn,codenexus::parse=debug`).
///
/// Invalid entries are reported to stderr and skipped, mirroring inklog's
/// env-override rule for illegal values ("warn, keep the existing value").
fn target_levels(spec: &str) -> HashMap<String, String> {
    let mut levels = HashMap::new();
    for entry in spec.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let parsed = entry
            .split_once('=')
            .filter(|(target, _)| !target.trim().is_empty())
            .and_then(|(target, level)| {
                canonical_level(level.trim()).map(|level| (target.trim(), level))
            });
        match parsed {
            Some((target, level)) => {
                levels.insert(target.to_string(), level.to_string());
            }
            None => {
                // Environment values are operator-controlled, but strip control
                // characters anyway: the entry is echoed to stderr and must not
                // smuggle terminal escapes (or split lines) into the output.
                let display: String = entry.chars().filter(|c| !c.is_control()).collect();
                eprintln!(
                    "{}",
                    crate::i18n::t("logging-invalid_target_level", &[("entry", display)])
                );
            }
        }
    }
    levels
}

/// Builds the inklog configuration for the CLI binaries from the `RUST_LOG`
/// and [`TARGET_LEVELS_ENV`] values (passed in so tests can pin them).
fn config(rust_log: &str, target_spec: &str) -> InklogConfig {
    let mut config = InklogConfig::default();
    config.global.level = global_level(rust_log).to_string();
    config.global.format = LOG_FORMAT.to_string();
    config.console_sink = Some(ConsoleSinkConfig {
        enabled: true,
        colored: true,
        stderr_levels: CONSOLE_STDERR_LEVELS
            .iter()
            .map(|level| (*level).to_string())
            .collect(),
        ..Default::default()
    });
    config.file_sink = Some(FileSinkConfig {
        enabled: true,
        path: log_file(),
        max_size: LOG_FILE_MAX_SIZE.to_string(),
        rotation_time: "daily".to_string(),
        keep_files: LOG_FILE_KEEP_FILES,
        compress: true,
        ..Default::default()
    });
    config.performance.channel_capacity = LOG_CHANNEL_CAPACITY;
    config.target_levels = target_levels(target_spec);
    config
}

/// Initializes the global inklog logger (tracing subscriber + `log` facade).
///
/// Reads `RUST_LOG` and [`TARGET_LEVELS_ENV`]; the first call installs the
/// logger, later calls are no-ops. Panics only if inklog's runtime or logger
/// setup fails (unusable `RUST_LOG` directives surface as the same failure as
/// before).
pub fn init_logging() {
    if LOGGER.get().is_some() {
        return;
    }
    let config = config(
        &std::env::var("RUST_LOG").unwrap_or_default(),
        &std::env::var(TARGET_LEVELS_ENV).unwrap_or_default(),
    );
    let runtime = tokio::runtime::Runtime::new().expect("create tokio runtime for inklog");
    let logger = runtime
        .block_on(LoggerManager::with_config(config))
        .expect("init inklog");
    // The runtime must outlive this call: the manager's sink workers run on
    // it, and the manager itself is kept in `LOGGER` for `set_level` (a
    // dropped manager shuts its workers down).
    std::mem::forget(runtime);
    let _ = LOGGER.set(logger);
}

/// Hot-tunes the installed logger's level without a restart.
///
/// `target = None` adjusts the global level, `Some(target)` one module's.
/// Level aliases (`warning`/`critical`/`fatal`) are normalized to the five
/// names the `log` facade and `EnvFilter` understand — inklog accepts the
/// aliases but the facade maps unknown names to `Off`, which would silence the
/// whole process. Targets are restricted to tracing-target characters so a
/// crafted value cannot smuggle `EnvFilter` separators (`=`, `,`, whitespace).
/// Errors if the logger is not initialized yet or an argument is invalid; a
/// rejected call leaves the running directive set unchanged.
pub fn set_level(target: Option<&str>, level: &str) -> Result<(), String> {
    let logger = LOGGER
        .get()
        .ok_or_else(|| "logging is not initialized".to_string())?;
    let level = canonical_level(level).ok_or_else(|| format!("invalid log level: {level}"))?;
    if let Some(target) = target {
        if !valid_target(target) {
            return Err(format!("invalid log target: {target}"));
        }
    }
    logger.set_level(target, level).map_err(|e| e.to_string())
}

/// Whether `target` is safe to embed in an `EnvFilter` directive: non-empty,
/// bounded, and limited to the characters tracing targets are built from.
fn valid_target(target: &str) -> bool {
    !target.is_empty()
        && target.len() <= 128
        && target
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.'))
}

/// Drains queued records before a one-shot CLI exits and stops the sink
/// workers.
///
/// inklog's sinks are asynchronous: without an explicit drain, a fast command
/// exits before its tail records (e.g. the error-level `fn_exit` written by
/// `#[forge::log]`) reach the file sink. Waits (bounded) for the queue to
/// empty, then shuts the workers down. Long-running modes (MCP / `webui`)
/// never call this.
pub fn flush() {
    let Some(logger) = LOGGER.get() else {
        return;
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
    while logger.channel_len() > 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let _ = logger.shutdown();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sets an environment variable for one test and restores the previous
    /// value on drop. Tests that use it must be `serial` (see the named
    /// `logging_env` group) because the process environment is global.
    struct EnvGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn global_level_takes_last_plain_level_only() {
        assert_eq!(global_level(""), "info");
        assert_eq!(global_level("debug"), "debug");
        assert_eq!(global_level("trace,debug"), "debug");
        assert_eq!(global_level("debug,codenexus=trace"), "debug");
        assert_eq!(global_level("codenexus=debug,sqlx=warn"), "info");
        assert_eq!(global_level("warning"), "warn");
        assert_eq!(global_level("not-a-level"), "info");
        assert_eq!(global_level(" debug "), "debug");
    }

    #[test]
    fn target_levels_parses_pairs_and_skips_invalid_entries() {
        assert_eq!(target_levels(""), HashMap::new());
        assert_eq!(
            target_levels("hyper=warn, codenexus::parse=debug"),
            HashMap::from([
                ("hyper".to_string(), "warn".to_string()),
                ("codenexus::parse".to_string(), "debug".to_string()),
            ])
        );
        // later duplicate wins
        assert_eq!(
            target_levels("hyper=warn,hyper=error"),
            HashMap::from([("hyper".to_string(), "error".to_string())])
        );
        // aliases normalize to EnvFilter levels
        assert_eq!(
            target_levels("a=warning,b=critical"),
            HashMap::from([
                ("a".to_string(), "warn".to_string()),
                ("b".to_string(), "error".to_string()),
            ])
        );
    }

    #[test]
    fn target_levels_never_panics_on_invalid_entries() {
        assert_eq!(target_levels("hyper"), HashMap::new());
        assert_eq!(target_levels("hyper=verbose"), HashMap::new());
        assert_eq!(target_levels("=warn"), HashMap::new());
        assert_eq!(
            target_levels("bad,hyper=warn"),
            HashMap::from([("hyper".to_string(), "warn".to_string())])
        );
    }

    #[test]
    fn config_carries_every_previous_builder_knob() {
        let config = config("debug", "");
        assert_eq!(config.global.level, "debug");
        assert_eq!(config.global.format, LOG_FORMAT);

        let console = config.console_sink.as_ref().expect("console sink enabled");
        assert!(console.enabled);
        assert!(console.colored);
        assert_eq!(
            console.stderr_levels,
            CONSOLE_STDERR_LEVELS
                .iter()
                .map(|level| level.to_string())
                .collect::<Vec<_>>()
        );

        let file = config.file_sink.as_ref().expect("file sink enabled");
        assert!(file.enabled);
        assert_eq!(file.path, log_file());
        assert_eq!(file.max_size, LOG_FILE_MAX_SIZE);
        assert_eq!(file.rotation_time, "daily");
        assert_eq!(file.keep_files, LOG_FILE_KEEP_FILES);
        assert!(file.compress);

        assert_eq!(config.performance.channel_capacity, LOG_CHANNEL_CAPACITY);
        assert!(config.target_levels.is_empty());
    }

    #[test]
    fn valid_target_rejects_filter_separators() {
        assert!(valid_target("codenexus::parse"));
        assert!(valid_target("hyper"));
        assert!(valid_target("my-crate"));
        assert!(!valid_target(""));
        assert!(!valid_target("a=b"));
        assert!(!valid_target("a,b"));
        assert!(!valid_target("a b"));
        assert!(!valid_target(&"x".repeat(129)));
    }

    /// Level aliases must normalize before delegating to inklog: inklog's
    /// validation accepts them, but the `log` facade maps unknown names to
    /// `Off`, which would silence the whole process.
    #[test]
    #[serial_test::serial(logging_env)]
    fn hot_set_level_normalizes_aliases_and_rejects_bad_targets() {
        let _env = EnvGuard::set("RUST_LOG", "");
        init_logging();
        let logger = LOGGER.get().expect("init_logging must install the manager");

        set_level(None, "warning").expect("alias level must be accepted");
        assert_eq!(logger.current_level_filter_string(), "warn");
        assert!(set_level(Some("bad target=1"), "debug").is_err());
        assert!(set_level(Some("hyper"), "verbose").is_err());
        assert_eq!(
            logger.current_level_filter_string(),
            "warn",
            "rejected calls must leave the directive set unchanged"
        );
        set_level(None, "error").expect("restore quiet level for other tests");
    }

    /// Regression guard: a `RUST_LOG` directive list must never be forwarded
    /// as the global level (the builder validated it and aborted the build).
    #[test]
    fn config_global_level_is_always_a_plain_level() {
        for rust_log in [
            "",
            "debug",
            "codenexus=debug,sqlx=warn",
            "trace,codenexus=trace",
            "not-a-level",
        ] {
            let config = config(rust_log, "hyper=warn");
            assert!(
                LogLevel::is_valid_level(&config.global.level),
                "global level must stay a plain level name, got {:?} for RUST_LOG={rust_log:?}",
                config.global.level
            );
            assert_eq!(
                config.target_levels.get("hyper").map(String::as_str),
                Some("warn")
            );
        }
    }

    /// `RUST_LOG` directives and `CODENEXUS_LOG_TARGETS` presets must both
    /// reach the `EnvFilter`, with the global level kept as the fallback.
    #[tokio::test]
    #[serial_test::serial(logging_env)]
    async fn rust_log_directives_and_presets_reach_the_env_filter() {
        let _env = EnvGuard::set("RUST_LOG", "codenexus=debug,sqlx=warn");
        let (manager, _subscriber, filter) = LoggerManager::build_detached(config(
            &std::env::var("RUST_LOG").unwrap(),
            "hyper=warn",
        ))
        .await
        .expect("build_detached should succeed");

        assert_eq!(
            manager.current_level_filter_string(),
            "info,hyper=warn,codenexus=debug,sqlx=warn"
        );
        // EnvFilter's Display reorders directives, so compare semantics.
        let rendered = filter.to_string();
        for directive in ["info", "hyper=warn", "codenexus=debug", "sqlx=warn"] {
            assert!(
                rendered.contains(directive),
                "EnvFilter must carry {directive}, got: {rendered}"
            );
        }

        let _ = manager.shutdown();
    }

    /// Runtime hot-tuning upserts into the installed directive set; invalid
    /// levels are rejected without touching it. The global branch also writes
    /// the process-wide `log::max_level` (inklog's documented double-facade
    /// sync), which is why this test is serialized.
    #[tokio::test]
    #[serial_test::serial(logging_env)]
    async fn hot_set_level_upserts_directives_and_rejects_invalid() {
        let _env = EnvGuard::set("RUST_LOG", "");
        let (manager, _subscriber, _filter) =
            LoggerManager::build_detached(config("info", "hyper=warn"))
                .await
                .expect("build_detached should succeed");
        assert_eq!(manager.current_level_filter_string(), "info,hyper=warn");

        let set = |target: Option<&str>, level: &str| {
            manager.set_level(target, level).map_err(|e| e.to_string())
        };
        assert!(set(Some("hyper"), "trace").is_ok());
        assert_eq!(manager.current_level_filter_string(), "info,hyper=trace");
        assert!(set(Some("my_crate"), "debug").is_ok());
        assert_eq!(
            manager.current_level_filter_string(),
            "info,hyper=trace,my_crate=debug"
        );
        assert!(set(None, "debug").is_ok());
        assert_eq!(
            manager.current_level_filter_string(),
            "debug,hyper=trace,my_crate=debug"
        );

        let err = set(Some("hyper"), "not-a-level").unwrap_err();
        assert!(err.contains("Invalid log level"), "unexpected error: {err}");
        assert_eq!(
            manager.current_level_filter_string(),
            "debug,hyper=trace,my_crate=debug",
            "a rejected level must leave the directive set untouched"
        );

        let _ = manager.shutdown();
    }

    /// End-to-end: `init_logging` installs the logger retained for
    /// [`set_level`], and the public hot-tuning path reaches it.
    #[test]
    #[serial_test::serial(logging_env)]
    fn init_logging_retains_a_manager_for_hot_tuning() {
        let _env = EnvGuard::set("RUST_LOG", "");

        init_logging();
        // Second call is a no-op: the logger is installed once per process.
        init_logging();

        let logger = LOGGER.get().expect("init_logging must install the manager");
        // From here on this logger is the process-global subscriber: take it
        // to `error` immediately so the records other tests emit stay out of
        // the test output and the log file.
        set_level(None, "error").expect("hot-tuning the installed logger must succeed");
        assert_eq!(logger.current_level_filter_string(), "error");

        set_level(None, "debug").expect("tuning the level back up must succeed");
        assert_eq!(LOGGER.get().unwrap().current_level_filter_string(), "debug");
        assert!(set_level(Some("codenexus"), "invalid-level").is_err());
        set_level(None, "error").expect("a rejected level must not break later hot-tuning");
        assert_eq!(LOGGER.get().unwrap().current_level_filter_string(), "error");
    }
}
