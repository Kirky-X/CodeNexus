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
//!
//! Log-volume knobs follow the same env pattern: [`SAMPLE_ENV`] configures
//! inklog's sampling (which records survive under rate-limit pressure) and
//! [`RATE_LIMIT_ENV`] per-target-prefix token quotas. Both go through
//! `InklogConfig` — the config path validates up front and does not pass
//! through the builder-side sampling gap noted in the 0.3.13-rc.1 changelog.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use inklog::{
    ConsoleSinkConfig, FileSinkConfig, InklogConfig, LogLevel, LoggerManager, RateLimitConfig,
    SamplingConfig, TargetSamplingRule,
};

/// Global level used when `RUST_LOG` carries no plain level name.
const DEFAULT_LEVEL: &str = "info";

/// Per-target level presets: `target=level` pairs separated by commas.
const TARGET_LEVELS_ENV: &str = "CODENEXUS_LOG_TARGETS";

/// Sampling spec: `level=N` or `prefix=N[@level]` pairs separated by commas.
/// A key that names a valid level feeds inklog's per-level sampling; any
/// other key is a target prefix, with the optional `@level` suffix marking
/// records at or above it as always kept (`sqlx=100@warn`).
const SAMPLE_ENV: &str = "CODENEXUS_LOG_SAMPLE";

/// Per-target-prefix log quotas in tokens/second: `prefix=rate` pairs
/// separated by commas. Prefixes inklog matches longest-first; targets no
/// rule covers stay on the unthrottled global path.
const RATE_LIMIT_ENV: &str = "CODENEXUS_LOG_RATE_LIMIT";

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

/// One parsed [`SAMPLE_ENV`] entry: a level name resolves to per-level
/// sampling (levels are a closed set, so the check is unambiguous), anything
/// else is a target prefix.
enum SampleRule {
    Level(&'static str, u64),
    Prefix(String, TargetSamplingRule),
}

/// Parses one `key=N` / `key=N@level` sample entry; `None` marks it invalid.
fn parse_sample_entry(entry: &str) -> Option<SampleRule> {
    let (key, spec) = entry.split_once('=')?;
    let (every_n, keep_level) = match spec.split_once('@') {
        Some((n, keep)) => (n, Some(canonical_level(keep.trim())?)),
        None => (spec, None),
    };
    let every_n: u64 = every_n.trim().parse().ok()?;
    if every_n == 0 {
        return None;
    }
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    if let Some(level) = canonical_level(key) {
        return Some(SampleRule::Level(level, every_n));
    }
    Some(SampleRule::Prefix(
        key.to_string(),
        TargetSamplingRule {
            keep_level: keep_level.map(str::to_string),
            sample_every_n: every_n,
        },
    ))
}

/// Parses [`SAMPLE_ENV`] into inklog's sampling config. Invalid entries are
/// reported to stderr and skipped, mirroring [`target_levels`].
fn sample_config(spec: &str) -> SamplingConfig {
    let mut config = SamplingConfig::default();
    for entry in spec.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        match parse_sample_entry(entry) {
            Some(SampleRule::Level(level, every_n)) => {
                config.per_level.insert(level.to_string(), every_n);
            }
            Some(SampleRule::Prefix(prefix, rule)) => {
                config.per_target_prefix.insert(prefix, rule);
            }
            None => {
                let display: String = entry.chars().filter(|c| !c.is_control()).collect();
                eprintln!(
                    "{}",
                    crate::i18n::t("logging-invalid_sample_entry", &[("entry", display)])
                );
            }
        }
    }
    config
}

/// Parses one `prefix=rate` entry; `None` marks it invalid. Kept symmetric
/// with [`parse_sample_entry`] so all three env parsers share one shape.
fn parse_rate_limit_entry(entry: &str) -> Option<(String, u64)> {
    let (prefix, rate) = entry.split_once('=')?;
    let rate: u64 = rate.trim().parse().ok()?;
    (rate > 0 && !prefix.trim().is_empty()).then(|| (prefix.trim().to_string(), rate))
}

/// Parses [`RATE_LIMIT_ENV`] into inklog's per-target-prefix quotas, with the
/// same invalid-entry discipline as [`sample_config`].
fn rate_limit_config(spec: &str) -> RateLimitConfig {
    let mut config = RateLimitConfig::default();
    for entry in spec.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        match parse_rate_limit_entry(entry) {
            Some((prefix, rate)) => {
                config.rules.insert(prefix, rate);
            }
            None => {
                let display: String = entry.chars().filter(|c| !c.is_control()).collect();
                eprintln!(
                    "{}",
                    crate::i18n::t("logging-invalid_rate_limit_entry", &[("entry", display)])
                );
            }
        }
    }
    config
}

/// The four env specs [`config`] consumes, read from the process environment
/// by [`LogEnv::from_env`] and pinned field-by-field by tests. Three of the
/// four are same-typed comma specs — a struct keeps them from being swapped
/// silently at a call site the way positional `&str` parameters would.
#[derive(Debug, Clone, Default)]
struct LogEnv {
    rust_log: String,
    targets: String,
    sample: String,
    rate_limit: String,
}

impl LogEnv {
    fn from_env() -> Self {
        Self {
            rust_log: std::env::var("RUST_LOG").unwrap_or_default(),
            targets: std::env::var(TARGET_LEVELS_ENV).unwrap_or_default(),
            sample: std::env::var(SAMPLE_ENV).unwrap_or_default(),
            rate_limit: std::env::var(RATE_LIMIT_ENV).unwrap_or_default(),
        }
    }
}

/// Builds the inklog configuration for the CLI binaries from the specs in
/// `env`.
fn config(env: &LogEnv) -> InklogConfig {
    let mut config = InklogConfig::default();
    config.global.level = global_level(&env.rust_log).to_string();
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
    config.target_levels = target_levels(&env.targets);
    config.sampling = sample_config(&env.sample);
    config.rate_limit = rate_limit_config(&env.rate_limit);
    config
}

/// Initializes the global inklog logger (tracing subscriber + `log` facade).
///
/// Reads `RUST_LOG`, [`TARGET_LEVELS_ENV`], [`SAMPLE_ENV`] and
/// [`RATE_LIMIT_ENV`]; the first call installs the logger, later calls are
/// no-ops. Panics only if inklog's runtime or logger setup fails (unusable
/// `RUST_LOG` directives surface as the same failure as before).
pub fn init_logging() {
    if LOGGER.get().is_some() {
        return;
    }
    let config = config(&LogEnv::from_env());
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
        let config = config(&LogEnv {
            rust_log: "debug".to_string(),
            ..Default::default()
        });
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
        assert!(config.sampling.is_empty());
        assert!(config.rate_limit.is_empty());
    }

    /// `CODENEXUS_LOG_SAMPLE`: level keys land in `per_level`, other keys in
    /// `per_target_prefix` (with the `@level` keep exemption); invalid
    /// entries are skipped rather than poisoning the whole config — inklog
    /// hard-rejects invalid sampling configs, so the parser must never
    /// forward one.
    #[test]
    fn sample_config_splits_levels_from_target_prefixes() {
        let config = sample_config("debug=10, sqlx=100@warn, warning=5");
        assert_eq!(
            config.per_level,
            HashMap::from([
                ("debug".to_string(), 10),
                ("warn".to_string(), 5), // alias normalized
            ])
        );
        let rule = config
            .per_target_prefix
            .get("sqlx")
            .expect("sqlx rule present");
        assert_eq!(rule.sample_every_n, 100);
        assert_eq!(rule.keep_level.as_deref(), Some("warn"));

        let plain = sample_config("lbug=3");
        assert!(plain.per_level.is_empty());
        let rule = plain.per_target_prefix.get("lbug").expect("prefix rule");
        assert_eq!(rule.sample_every_n, 3);
        assert_eq!(rule.keep_level, None);

        for bad in [
            "debug",      // no '='
            "=10",        // empty key
            "debug=0",    // rate must be >= 1
            "sqlx=10@no", // invalid keep level
            "sqlx=x",     // non-numeric rate
        ] {
            let config = sample_config(bad);
            assert!(
                config.per_level.is_empty() && config.per_target_prefix.is_empty(),
                "invalid entry {bad:?} must be dropped"
            );
        }
    }

    /// `CODENEXUS_LOG_RATE_LIMIT`: `prefix=rate` pairs; zero or non-numeric
    /// rates and empty prefixes are skipped (inklog hard-rejects them).
    #[test]
    fn rate_limit_config_parses_prefix_quotas_and_skips_invalid() {
        let config = rate_limit_config("sqlx=50, codenexus::daemon=200");
        assert_eq!(
            config.rules,
            HashMap::from([
                ("sqlx".to_string(), 50),
                ("codenexus::daemon".to_string(), 200),
            ])
        );
        for bad in ["sqlx", "=50", "sqlx=0", "sqlx=fast"] {
            assert!(
                rate_limit_config(bad).rules.is_empty(),
                "invalid entry {bad:?} must be dropped"
            );
        }
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
            let config = config(&LogEnv {
                rust_log: rust_log.to_string(),
                targets: "hyper=warn".to_string(),
                ..Default::default()
            });
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
        let (manager, _subscriber, filter) = LoggerManager::build_detached(config(&LogEnv {
            rust_log: std::env::var("RUST_LOG").unwrap(),
            targets: "hyper=warn".to_string(),
            ..Default::default()
        }))
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
        let (manager, _subscriber, _filter) = LoggerManager::build_detached(config(&LogEnv {
            rust_log: "info".to_string(),
            targets: "hyper=warn".to_string(),
            ..Default::default()
        }))
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

    /// The `main.rs` comment names the file-sink compression backend, so it
    /// must name the backend inklog is actually built with. `Cargo.lock` is
    /// the resolved truth: inklog's locked dependencies carry `zstd` when the
    /// `compression` feature is enabled (also transitively pulled through
    /// dependency feature unification) and `flate2` when `gzip` is — the
    /// two backends are mutually exclusive in the final binary.
    #[test]
    fn logging_comment_names_the_backend_inklog_is_built_with() {
        let main_rs = include_str!("main.rs");
        let lock = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock"),
        )
        .expect("read Cargo.lock");

        // The `dependencies = [...]` block of inklog's lock entry.
        let entry = lock
            .find("name = \"inklog\"")
            .expect("Cargo.lock must carry an inklog entry");
        let block = &lock[entry..];
        let block = &block[..block.find(']').expect("dependency list closes")];
        let zstd_backend = block.contains("\"zstd\"");
        let gzip_backend = block.contains("\"flate2\"");
        assert!(
            zstd_backend != gzip_backend,
            "inklog must resolve to exactly one compression backend, got zstd={zstd_backend} gzip={gzip_backend}"
        );

        // The `// Global inklog backend: ...` comment above init_logging().
        let comment = main_rs
            .lines()
            .skip_while(|l| !l.contains("Global inklog backend"))
            .take_while(|l| !l.contains("init_logging();"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !comment.is_empty(),
            "main.rs must narrate the inklog backend above init_logging()"
        );

        if gzip_backend {
            assert!(
                comment.contains("gzip"),
                "inklog is built with the gzip (flate2) backend, so the comment must say so: {comment}"
            );
        } else {
            assert!(
                !comment.contains("gzip"),
                "inklog is built with the zstd backend (no flate2 in its lock entry), so the comment must not promise gzip: {comment}"
            );
        }
        if zstd_backend {
            assert!(
                comment.contains("zstd"),
                "inklog's compression feature pulls zstd; the comment must name it: {comment}"
            );
        }
    }
}
