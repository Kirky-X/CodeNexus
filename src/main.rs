// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! CodeNexus binary entry point.
//!
//! CLI mode: sdforge `CliBuilder` dispatches to `#[forge(cli = true)]`
//! handlers registered via `inventory` in the service layer.
//!
//! MCP mode (gated by `mcp` feature): sdforge MCP server serves
//! `#[forge]` tools over stdio via rmcp.

use std::collections::HashMap;
use std::path::PathBuf;

use codenexus::kit::{build_kit, KitBootstrapConfig, KitError};
use codenexus::service::error::CodeNexusError;
use codenexus::service::init_kit;
use codenexus::storage::StorageError;

/// Default directory (relative to CWD) that holds per-project database files.
const DEFAULT_DB_DIR: &str = ".codenexus";

/// Fallback project name used when no command-specific `name`/`path` arg is
/// available to derive one (e.g. non-indexing subcommands).
const FALLBACK_PROJECT_NAME: &str = "codenexus";

// `DEFAULT_DEBOUNCE_MS` is sourced from the daemon module when the `daemon`
// feature is enabled, or from the kit bootstrap fallback otherwise — avoiding
// a third hardcoded copy of the 2000ms default (BR-DAEMON-001).
#[cfg(feature = "daemon")]
use codenexus::daemon::DEFAULT_DEBOUNCE_MS;
#[cfg(not(feature = "daemon"))]
use codenexus::kit::bootstrap::DEFAULT_DEBOUNCE_MS;

/// Initialize the global `tracing` subscriber using inklog as the sole backend.
///
/// Configures console (colored) + file output with daily rotation, 100 MB max
/// file size, gzip compression, and 30-day retention. The log level is read
/// from `RUST_LOG` (default: `info`).
pub fn init_logging() {
    init_inklog();
}

fn init_inklog() {
    let level = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());

    let rt = tokio::runtime::Runtime::new().expect("create tokio runtime for inklog");
    let logger = rt
        .block_on(async {
            inklog::LoggerManager::builder()
                .level(&level)
                .format("{timestamp} [{level}] {target} - {message}")
                .console(true)
                .console_colored(true)
                .file("logs/codenexus.log")
                .file_max_size("100MB")
                .file_compress(true)
                .file_rotation_time("daily")
                .file_keep_files(30)
                .channel_capacity(10000)
                .build()
                .await
        })
        .expect("init inklog");

    std::mem::forget(logger);
    std::mem::forget(rt);
}

fn main() {
    init_logging();

    #[cfg(feature = "mcp")]
    if std::env::args().nth(1) == Some("mcp".into()) {
        run_mcp_server();
        return;
    }

    run_cli();
}

/// CLI mode: build sdforge Command, parse args, dispatch to service handler.
fn run_cli() {
    let cmd = sdforge::cli::CliBuilder::new()
        .with_name("codenexus")
        .with_global_arg(
            sdforge::cli::GlobalArg::new("db")
                .long("db")
                .help("Database path (default: .codenexus/<project>.lbug)"),
        )
        .with_global_arg(
            sdforge::cli::GlobalArg::new("debounce-ms")
                .long("debounce-ms")
                .default_value(DEFAULT_DEBOUNCE_MS.to_string())
                .help("Daemon debounce interval (ms)"),
        )
        .build()
        // Override sdforge CliBuilder's injected version/about (builder.rs
        // hardcodes 0.4.2 from the sdforge crate). Cargo.toml is the single
        // source of truth for the codenexus version — clap applies the last
        // call wins, so this overrides cleanly. See R-cli-002.
        .version(codenexus::version())
        .about("CodeNexus — Code Intelligence");

    // Inject sentinel default_values so users can omit optional parameters
    // on the command line (sdforge 0.4.2 marks all non-Option Body params
    // required=true with no default attribute). See `apply_sentinel_defaults`.
    let cmd = apply_sentinel_defaults(cmd);

    let matches = cmd.get_matches();

    let sub_name = match matches.subcommand_name() {
        Some(name) => name,
        None => {
            // No subcommand provided — print hint to stdout and exit 0
            // so callers can detect "no-op" via exit code (rule 12).
            println!("Use --help to see available commands");
            std::process::exit(0);
        }
    };
    let sub_matches = matches.subcommand_matches(sub_name).unwrap();

    let db = extract_global_arg(&matches, sub_matches, "db", &default_db_path(sub_matches));
    let debounce_ms = extract_global_arg(
        &matches,
        sub_matches,
        "debounce-ms",
        &DEFAULT_DEBOUNCE_MS.to_string(),
    )
    .parse()
    .unwrap_or_else(|_| {
        eprintln!("[warn] Invalid --debounce-ms value, using default ({DEFAULT_DEBOUNCE_MS}ms)");
        DEFAULT_DEBOUNCE_MS
    });

    // Create the tokio runtime early — build_kit is async (AsyncKit migration)
    // and the same runtime is reused for handler execution below.
    let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    // Build and init Kit. Non-fatal: setup/lsp commands don't need Kit.
    //
    // Ensure the default DB directory exists so `Database::new` can create the
    // file (it does not create parent directories itself).
    if let Some(parent) = PathBuf::from(&db).parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("[warn] Failed to create DB directory {parent:?}: {e}");
            }
        }
    }

    // Gate read-only commands against a missing DB file. Without this,
    // `list --db <missing>` would build an empty DB (create-on-open) and
    // return `[]` with exit 0 — a silent success. Creating commands bypass.
    if let Err(msg) = validate_db_exists(&db, sub_name) {
        eprintln!("[error] {msg}");
        std::process::exit(4);
    }

    // Read-only commands open the DB read-only so multiple processes can read
    // concurrently (DuckDB/LadybugDB shared-read); writing commands keep RW.
    let config = KitBootstrapConfig::new(PathBuf::from(&db))
        .with_debounce_ms(debounce_ms)
        .with_read_only(requires_existing_db(sub_name));
    match runtime.block_on(build_kit(&config)) {
        Ok(kit) => {
            if let Err(e) = init_kit(kit) {
                eprintln!("[warn] Kit already initialized: {e}");
            }
        }
        Err(e) => {
            // Rule 12: a DB lock conflict must surface as exit 2 with a clear
            // message, not hide behind the generic kit_not_initialized exit 1.
            if let Some(hint) = extract_db_locked_hint(&e) {
                eprintln!("[error] 数据库被锁定，无法打开：{hint}");
                eprintln!("  另一个 codenexus 进程可能正在写入（index/import 会独占 DB）。");
                eprintln!("  请关闭其他 codenexus 进程后重试，或等待其完成。");
                std::process::exit(2);
            }
            eprintln!("[warn] Failed to build Kit (commands requiring Kit will fail): {e}");
        }
    }

    let args = extract_args(sub_name, sub_matches);

    let handler = match sdforge::inventory::iter::<sdforge::cli::CliHandlerRegistration>()
        .find(|h| h.name == sub_name)
    {
        Some(h) => h,
        None => {
            eprintln!("[error] No handler registered for command: {sub_name}");
            std::process::exit(1);
        }
    };

    if let Err(api_error) = runtime.block_on((handler.handler)(args)) {
        let cli_error = CodeNexusError::from(api_error);
        eprintln!("Error: {cli_error}");
        std::process::exit(cli_error.exit_code());
    }
}

/// Extracts a global arg value from top-level or subcommand matches.
fn extract_global_arg(
    top: &sdforge::clap::ArgMatches,
    sub: &sdforge::clap::ArgMatches,
    name: &str,
    fallback: &str,
) -> String {
    top.get_one::<String>(name)
        .or_else(|| sub.get_one::<String>(name))
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

/// Computes the default database path when `--db` is not specified.
///
/// Resolves to `.codenexus/<project>.lbug`, where `<project>` is derived from
/// the subcommand's `name` arg when present, otherwise the directory name of
/// its `path` arg, otherwise [`FALLBACK_PROJECT_NAME`]. The project component
/// is sanitized so it is safe to use as a single filesystem path segment.
fn default_db_path(sub: &sdforge::clap::ArgMatches) -> String {
    // try_get_one (not get_one): commands like `search`/`impact`/`context` do
    // not define a `name`/`path` arg, and clap's get_one panics on an
    // undefined arg id. Fall back gracefully instead.
    let raw_project = sub
        .try_get_one::<String>("name")
        .ok()
        .flatten()
        .cloned()
        .or_else(|| {
            sub.try_get_one::<String>("path").ok().flatten().map(|p| {
                std::path::Path::new(p)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(FALLBACK_PROJECT_NAME)
                    .to_string()
            })
        })
        .unwrap_or_else(|| FALLBACK_PROJECT_NAME.to_string());

    let project = sanitize_project_name(&raw_project);
    format!("{DEFAULT_DB_DIR}/{project}.lbug")
}

/// Reduces `name` to a safe single path segment: trims whitespace, replaces
/// path separators and control characters, collapses runs, and falls back to
/// [`FALLBACK_PROJECT_NAME`] when empty.
fn sanitize_project_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_whitespace() || c == '/' || c == '\\' {
                '_'
            } else {
                c
            }
        })
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
        .collect();
    if cleaned.is_empty() {
        FALLBACK_PROJECT_NAME.to_string()
    } else {
        cleaned
    }
}

/// Read-only commands that need an existing database. `index`/`import` create
/// one (LadybugDB create-on-open); `setup`/`lsp_*`/`hook`/`mcp`/`verify` don't
/// query the graph. Without this gate, `list --db <missing>` silently builds
/// an empty DB and returns `[]` with exit 0 — a silent success.
fn requires_existing_db(sub_name: &str) -> bool {
    matches!(
        sub_name,
        "list" | "search" | "query" | "impact" | "context" | "trace"
    )
}

/// Walks the [`KitError`] source chain for [`StorageError::DatabaseLocked`]
/// (Rule 12). Returns the holder hint so the CLI can exit 2 with a clear
/// message instead of falling through to the generic `kit_not_initialized`
/// exit 1 when another process holds the DB write lock.
fn extract_db_locked_hint(e: &KitError) -> Option<String> {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = current {
        if let Some(StorageError::DatabaseLocked { holder_hint }) =
            err.downcast_ref::<StorageError>()
        {
            return Some(holder_hint.clone());
        }
        current = err.source();
    }
    None
}

/// Returns `Err(msg)` when a read command targets a DB file that does not yet
/// exist. The caller exits with code 4 (NotFound). Creating commands bypass.
fn validate_db_exists(db: &str, sub_name: &str) -> Result<(), String> {
    if requires_existing_db(sub_name) && !PathBuf::from(db).exists() {
        Err(format!(
            "database not found: {db}\n  command '{sub_name}' needs an existing DB; run `codenexus index ...` first"
        ))
    } else {
        Ok(())
    }
}

/// Extracts subcommand args into a `HashMap<String, String>` for the handler.
fn extract_args(
    sub_name: &str,
    sub_matches: &sdforge::clap::ArgMatches,
) -> HashMap<String, String> {
    let mut args = HashMap::new();
    for reg in sdforge::inventory::iter::<sdforge::cli::CliCommandRegistration>() {
        if reg.name == sub_name {
            for arg_info in reg.args {
                if let Some(value) = sub_matches.get_one::<String>(arg_info.name) {
                    args.insert(arg_info.name.to_string(), value.clone());
                }
            }
            break;
        }
    }
    args
}

/// Sentinel default values for optional CLI parameters.
///
/// sdforge 0.4.2's `#[forge]` macro marks all non-`Option` Body parameters
/// as `required = true` and does not expose a `default` attribute (the macro
/// always passes `default = None` to `CliArgInfo::new`). This table applies
/// clap `default_value`s post-build so users can omit these parameters on
/// the command line; the service-layer wrappers treat the sentinel values
/// ("0" for u32, "" for String, "false" for bool) as "use built-in default".
///
/// This is a workaround for sdforge 0.4.2's lack of `Option<T>` CLI support
/// (the macro generates `s.parse::<Option<T>>()` which fails because
/// `Option<T>: FromStr` is not implemented). When sdforge fixes the
/// `Option<T>` parse bug, these parameters can become `Option<T>` directly
/// and this table can be removed.
const SENTINEL_DEFAULTS: &[(&str, &str, &str)] = &[
    // (command, arg, default_value)
    // — complexity: 25 threshold/flag params (project is Path, always required)
    ("complexity", "red_only", "false"),
    ("complexity", "sort_by_severity", "false"),
    ("complexity", "cyclomatic_green", "0"),
    ("complexity", "cyclomatic_yellow", "0"),
    ("complexity", "cyclomatic_red", "0"),
    ("complexity", "cognitive_green", "0"),
    ("complexity", "cognitive_yellow", "0"),
    ("complexity", "cognitive_red", "0"),
    ("complexity", "nesting_green", "0"),
    ("complexity", "nesting_yellow", "0"),
    ("complexity", "nesting_red", "0"),
    ("complexity", "func_length_green", "0"),
    ("complexity", "func_length_yellow", "0"),
    ("complexity", "func_length_red", "0"),
    ("complexity", "halstead_volume_green", "0"),
    ("complexity", "halstead_volume_yellow", "0"),
    ("complexity", "halstead_volume_red", "0"),
    ("complexity", "maintainability_green", "0"),
    ("complexity", "maintainability_yellow", "0"),
    ("complexity", "maintainability_red", "0"),
    ("complexity", "time_complexity_green", ""),
    ("complexity", "time_complexity_yellow", ""),
    ("complexity", "time_complexity_red", ""),
    ("complexity", "space_complexity_yellow", ""),
    ("complexity", "space_complexity_red", ""),
    // — community: resolution (empty = default 0.5)
    ("community", "resolution", ""),
    // — cross_service: protocol (empty = all protocols)
    ("cross_service", "protocol", ""),
    // — api_impact: endpoint (empty = all endpoints)
    ("api_impact", "endpoint", ""),
];

/// Applies sentinel `default_value`s to CLI parameters listed in
/// [`SENTINEL_DEFAULTS`].
///
/// For each `(command, arg, default)` entry, finds the matching subcommand
/// and uses clap's `mut_arg` to set `required(false)` + `default_value`.
/// This lets users omit the parameter on the command line; the service
/// wrapper then receives the sentinel value and applies built-in defaults.
fn apply_sentinel_defaults(mut cmd: sdforge::clap::Command) -> sdforge::clap::Command {
    for sub in cmd.get_subcommands_mut() {
        let name = sub.get_name().to_string();
        for (cmd_name, arg_name, default) in SENTINEL_DEFAULTS {
            if &name != cmd_name {
                continue;
            }
            let arg_name = *arg_name;
            let default = *default;
            // mut_arg consumes self; take the subcommand out, modify, put back.
            let taken = std::mem::take(sub);
            *sub = taken.mut_arg(arg_name, |a| a.required(false).default_value(default));
        }
    }
    cmd
}

/// MCP mode: build Kit, serve sdforge MCP server over stdio.
#[cfg(feature = "mcp")]
fn run_mcp_server() {
    let db = std::env::args()
        .skip(2)
        .collect::<Vec<_>>()
        .chunks(2)
        .find(|chunk| chunk.len() == 2 && chunk[0] == "--db")
        .map(|chunk| chunk[1].clone())
        .unwrap_or_else(|| format!("{DEFAULT_DB_DIR}/{FALLBACK_PROJECT_NAME}.lbug"));

    // Create the tokio runtime early — build_kit is async (AsyncKit migration)
    // and the same runtime is reused for MCP serve below.
    let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    let kit = match runtime.block_on(build_kit(&KitBootstrapConfig::new(PathBuf::from(db)))) {
        Ok(kit) => kit,
        Err(e) => {
            eprintln!("[error] Failed to build Kit: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = init_kit(kit) {
        eprintln!("[error] Failed to init Kit: {e}");
        std::process::exit(1);
    }

    runtime.block_on(async {
        let server = sdforge::mcp::build();
        sdforge::mcp::serve_stdio(server)
            .await
            .expect("MCP serve error");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;
    use inklog::domain::core::LoggerSubscriber;
    use inklog::{LogRecord, Metrics};
    use std::sync::Arc;
    use tracing_subscriber::prelude::*;

    #[test]
    fn inklog_captures_tracing_events() {
        let (console_tx, console_rx) = unbounded::<Arc<LogRecord>>();
        let (async_tx, _async_rx) = unbounded::<Arc<LogRecord>>();
        let metrics = Arc::new(Metrics::new());
        let layer = LoggerSubscriber::new(console_tx, async_tx, metrics);
        let registry = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(registry, || {
            tracing::info!("codenexus_test_marker");
        });

        let record = console_rx
            .try_recv()
            .expect("should capture tracing event via inklog LoggerSubscriber");
        assert!(
            record.message.contains("codenexus_test_marker"),
            "expected message to contain the marker, got: {:?}",
            record.message
        );
    }

    // --- init_logging ---

    #[test]
    fn init_logging_does_not_panic() {
        init_logging();
    }

    // --- extract_global_arg ---

    fn build_test_cmd() -> sdforge::clap::Command {
        sdforge::clap::Command::new("codenexus")
            .arg(
                sdforge::clap::Arg::new("db")
                    .long("db")
                    .global(true)
                    .default_value("./codenexus.lbug"),
            )
            .subcommand(sdforge::clap::Command::new("index"))
    }

    #[test]
    fn extract_global_arg_returns_top_value_when_present() {
        let cmd = build_test_cmd();
        let matches = cmd.get_matches_from(["codenexus", "--db", "/top/db", "index"]);
        let sub = matches.subcommand_matches("index").unwrap();
        let result = extract_global_arg(&matches, sub, "db", "fallback");
        assert_eq!(result, "/top/db");
    }

    #[test]
    fn extract_global_arg_returns_sub_value_when_top_absent() {
        let cmd = sdforge::clap::Command::new("codenexus")
            .arg(sdforge::clap::Arg::new("db").long("db").global(true))
            .subcommand(
                sdforge::clap::Command::new("index").arg(sdforge::clap::Arg::new("db").long("db")),
            );
        let matches = cmd.get_matches_from(["codenexus", "index", "--db", "/sub/db"]);
        let sub = matches.subcommand_matches("index").unwrap();
        let result = extract_global_arg(&matches, sub, "db", "fallback");
        assert_eq!(result, "/sub/db");
    }

    #[test]
    fn extract_global_arg_returns_fallback_when_neither_present() {
        let cmd = sdforge::clap::Command::new("codenexus")
            .arg(
                sdforge::clap::Arg::new("unused")
                    .long("unused")
                    .global(true),
            )
            .subcommand(sdforge::clap::Command::new("index"));
        let matches = cmd.get_matches_from(["codenexus", "index"]);
        let sub = matches.subcommand_matches("index").unwrap();
        let result = extract_global_arg(&matches, sub, "unused", "fallback_value");
        assert_eq!(result, "fallback_value");
    }

    // --- default_db_path / sanitize_project_name ---

    fn index_sub_with(name: Option<&str>, path: Option<&str>) -> sdforge::clap::Command {
        let mut sub = sdforge::clap::Command::new("index")
            .arg(sdforge::clap::Arg::new("name").long("name"))
            .arg(sdforge::clap::Arg::new("path").long("path"));
        if let Some(n) = name {
            let v = n.to_string();
            sub = sub.mut_arg("name", move |a| a.default_value(v));
        }
        if let Some(p) = path {
            let v = p.to_string();
            sub = sub.mut_arg("path", move |a| a.default_value(v));
        }
        sdforge::clap::Command::new("codenexus").subcommand(sub)
    }

    #[test]
    fn default_db_path_uses_name_arg() {
        let cmd = index_sub_with(Some("my-project"), None);
        let m = cmd.get_matches_from(["codenexus", "index", "--name", "my-project"]);
        let sub = m.subcommand_matches("index").unwrap();
        assert_eq!(default_db_path(sub), ".codenexus/my-project.lbug");
    }

    #[test]
    fn default_db_path_falls_back_to_path_dirname() {
        let cmd = index_sub_with(None, Some("/home/user/CodeNexus"));
        let m = cmd.get_matches_from(["codenexus", "index", "--path", "/home/user/CodeNexus"]);
        let sub = m.subcommand_matches("index").unwrap();
        assert_eq!(default_db_path(sub), ".codenexus/CodeNexus.lbug");
    }

    #[test]
    fn default_db_path_falls_back_when_no_args() {
        let cmd = index_sub_with(None, None);
        let m = cmd.get_matches_from(["codenexus", "index"]);
        let sub = m.subcommand_matches("index").unwrap();
        assert_eq!(default_db_path(sub), ".codenexus/codenexus.lbug");
    }

    #[test]
    fn default_db_path_does_not_panic_when_subcommand_has_no_name_arg() {
        // Real commands like `search`/`impact`/`context` do not define a `name`
        // or `path` arg. default_db_path must fall back to the fallback project
        // name instead of panicking. Regression: clap `get_one` panics on an
        // arg id that the Command never defined.
        let sub_cmd = sdforge::clap::Command::new("search");
        let cmd = sdforge::clap::Command::new("codenexus").subcommand(sub_cmd);
        let m = cmd.get_matches_from(["codenexus", "search"]);
        let sub = m.subcommand_matches("search").unwrap();
        assert_eq!(default_db_path(sub), ".codenexus/codenexus.lbug");
    }

    // --- requires_existing_db / validate_db_exists ---

    #[test]
    fn validate_db_exists_errors_for_read_command_when_db_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("nope.lbug");
        let err = validate_db_exists(missing.to_str().unwrap(), "list").unwrap_err();
        assert!(err.contains("database not found"), "got: {err}");
    }

    #[test]
    fn validate_db_exists_allows_index_to_create_new_db() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("new.lbug");
        assert!(validate_db_exists(missing.to_str().unwrap(), "index").is_ok());
    }

    #[test]
    fn validate_db_exists_ok_when_db_present() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = dir.path().join("exists.lbug");
        std::fs::write(&db, b"x").unwrap();
        assert!(validate_db_exists(db.to_str().unwrap(), "list").is_ok());
    }

    #[test]
    fn sanitize_project_name_strips_path_separators() {
        assert_eq!(sanitize_project_name("a/b\\c"), "a_b_c");
        assert_eq!(sanitize_project_name("  "), "codenexus");
        assert_eq!(sanitize_project_name("weird*name!"), "weirdname");
    }

    // --- extract_args ---

    #[test]
    fn extract_args_returns_empty_for_unregistered_command() {
        let cmd = sdforge::clap::Command::new("codenexus")
            .subcommand(sdforge::clap::Command::new("nonexistent_cmd"));
        let matches = cmd.get_matches_from(["codenexus", "nonexistent_cmd"]);
        let sub = matches.subcommand_matches("nonexistent_cmd").unwrap();
        let args = extract_args("nonexistent_cmd", sub);
        assert!(
            args.is_empty(),
            "unregistered command should return empty args"
        );
    }
}
