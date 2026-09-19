// Copyright (c) 2026 Kirky.X🌠
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
// a third hardcoded copy of the 2000ms default.
#[cfg(feature = "daemon")]
use codenexus::daemon::DEFAULT_DEBOUNCE_MS;
#[cfg(not(feature = "daemon"))]
use codenexus::kit::bootstrap::DEFAULT_DEBOUNCE_MS;

/// Initialize the global `tracing` subscriber using inklog as the sole backend.
///
/// Configures console (colored, stderr-routed) + file output with daily
/// rotation, 100 MB max file size, gzip compression, 30-day retention, and
/// inklog's built-in PII masking (`pii_masking_enabled` defaults to `true`
/// on the file sink). The log level is read from `RUST_LOG` (default: `info`).
///
/// All console records — `info` included — are routed to stderr so stdout
/// stays a pure JSON channel under `> out.json` redirection. The file sink
/// lives under `.codenexus/logs/` (not a bare `logs/` in the CWD) so any
/// command creates only the one side-effect directory users gitignore.
///
/// Note: file-sink *sampling* (`inklog::SamplingSink`) is intentionally not
/// wired yet — it requires the builder's `add_sink` path, whose
/// `build_with_deps` constructor does not install the global tracing/log
/// frontend (only the pure-config `with_config_and_sinks` path does), which
/// would silence all records. Revisit after the upstream inklog fix.
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
                // Route every log level to stderr so stdout carries only the
                // command's JSON output. inklog's default stderr_levels is
                // ["error", "warn"] (info goes to stdout), which corrupted
                // `codenexus index > result.json` whenever the index pipeline
                // emitted info! events (index_started/completed/performance).
                // Unknown level names in the list never match and are harmless.
                .console_stderr_levels(&["error", "warn", "info", "debug", "trace", "fatal"])
                // Keep the file sink inside .codenexus/ so a one-off command
                // in any project creates only the side-effect directory users
                // already gitignore. inklog's file sink create_dir_all's the
                // parent, so no mkdir here (see init_logging doc comment).
                .file(format!("{DEFAULT_DB_DIR}/logs/codenexus.log"))
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
    // Message i18n (daemon logs, CLI/user-visible strings): detect the
    // locale once up front. Lazy fallback inside i18n::t/tr guarantees
    // localized output even if this were skipped.
    codenexus::i18n::init();

    init_logging();

    #[cfg(feature = "mcp")]
    if std::env::args().nth(1) == Some("mcp".into()) {
        run_mcp_server();
        return;
    }

    run_cli();
}

/// Builds the full CLI `Command`: sdforge subcommands from
/// `#[forge(cli = true)]` inventory registrations + global args + sentinel
/// defaults. Extracted from `run_cli` so tests can assert on flag defaults.
fn build_cli_command() -> sdforge::clap::Command {
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
        // call wins, so this overrides cleanly.
        .version(codenexus::version())
        .about("CodeNexus — Code Intelligence")
        // Boolean --verbose flag (CliBuilder GlobalArg only supports value
        // args). Wired to the trait-kit `daemon.verbose-events` toggle.
        .arg(
            sdforge::clap::Arg::new("verbose")
                .long("verbose")
                .action(sdforge::clap::ArgAction::SetTrue)
                .global(true)
                .help("Emit debug diagnostics for each daemon file-event batch"),
        );

    // Inject sentinel default_values so users can omit optional parameters
    // on the command line (sdforge 0.4.2 marks all non-Option Body params
    // required=true with no default attribute). See `apply_sentinel_defaults`.
    apply_sentinel_defaults(cmd)
}

/// CLI mode: build sdforge Command, parse args, dispatch to service handler.
fn run_cli() {
    let cmd = build_cli_command();

    let matches = cmd.get_matches();

    let sub_name = match matches.subcommand_name() {
        Some(name) => name,
        None => {
            // No subcommand provided — print a short quick-start to stdout
            // and exit 0 so callers can detect "no-op" via exit code.
            println!(
                "CodeNexus {} — code knowledge graph CLI",
                codenexus::version()
            );
            println!();
            println!("Quick start:");
            println!("  codenexus index --path ./myrepo --name myrepo  # index a repository");
            println!("  codenexus query --cypher 'MATCH (f:Function) RETURN f.name LIMIT 10'");
            println!("  codenexus setup  # wire MCP into Claude Code / Cursor / Codex");
            println!();
            println!(
                "Run `codenexus --help` to list all commands, or `codenexus <command> --help` for its flags."
            );
            std::process::exit(0);
        }
    };
    let sub_matches = matches.subcommand_matches(sub_name).unwrap();

    // Load the per-project config file (.codenexus/config.json). Non-fatal:
    // shape warnings print to stderr and built-in defaults apply. Precedence
    // everywhere below: explicit CLI flag > config file > built-in default.
    let (project_config, config_warnings) = ProjectConfig::load();
    for warning in &config_warnings {
        eprintln!("[warn] {DEFAULT_DB_DIR}/config.json: {warning}");
    }

    let db = explicit_cli_arg(&matches, sub_matches, "db").unwrap_or_else(|| {
        let derived = default_db_path(sub_matches);
        // general.db only fills the "no signal" case: derivation fell through
        // to the fallback name AND no single .lbug file was discovered. When
        // discovery succeeded, `derived` already is the discovered path and
        // must not be overridden by the config.
        if derived == format!("{DEFAULT_DB_DIR}/{FALLBACK_PROJECT_NAME}.lbug")
            && discover_single_indexed_db().is_none()
        {
            if let Some(configured) = project_config.db.clone() {
                return configured;
            }
        }
        derived
    });
    let debounce_ms = explicit_cli_arg(&matches, sub_matches, "debounce-ms")
        .or_else(|| project_config.debounce_ms.map(|v| v.to_string()))
        .unwrap_or_else(|| DEFAULT_DEBOUNCE_MS.to_string())
        .parse()
        .unwrap_or_else(|_| {
            eprintln!(
                "[warn] Invalid --debounce-ms value, using default ({DEFAULT_DEBOUNCE_MS}ms)"
            );
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

    // --fresh: delete existing DB file before Kit init to reclaim space.
    // DuckDB DELETE doesn't reclaim pages and LadybugDB doesn't support
    // VACUUM, so repeated `--force` indexes accumulate dead space. --fresh
    // drops the old file so Kit opens a fresh one. See [`handle_fresh_flag`].
    handle_fresh_flag(sub_name, sub_matches, &db);

    // clean --rebuild: delete the whole DB file (schema-version mismatch or
    // corruption self-healing) before Kit init — deleting requires no open
    // handles, which is exactly the state a corrupt DB needs. See
    // [`handle_clean_rebuild`].
    handle_clean_rebuild(sub_name, sub_matches, &db);

    // Gate read-only commands against a missing DB file. Without this,
    // `list --db <missing>` would build an empty DB (create-on-open) and
    // return `[]` with exit 0 — a silent success. Creating commands bypass.
    if let Err(msg) = validate_db_exists(&db, sub_name) {
        eprintln!("[error] {msg}");
        std::process::exit(4);
    }

    // Read-only commands open the DB read-only so multiple processes can read
    // concurrently (DuckDB/LadybugDB shared-read); writing commands keep RW.
    // with_notify_webhook 仅在 daemon+hub 下存在（见 kit/bootstrap.rs）
    let config = KitBootstrapConfig::new(PathBuf::from(&db))
        .with_debounce_ms(debounce_ms)
        .with_read_only(opens_read_only(sub_name))
        .with_verbose(
            matches.value_source("verbose")
                == Some(sdforge::clap::parser::ValueSource::CommandLine)
                || project_config.verbose.unwrap_or(false),
        );
    #[cfg(all(feature = "daemon", feature = "hub"))]
    let config = config.with_notify_webhook(project_config.notify_webhook.clone());
    match runtime.block_on(build_kit(&config)) {
        Ok(kit) => {
            if let Err(e) = init_kit(kit) {
                eprintln!("[warn] Kit already initialized: {e}");
            }
        }
        Err(e) => {
            // A DB lock conflict must surface as exit 2 with a clear
            // message, not hide behind the generic kit_not_initialized exit 1.
            if let Some(hint) = extract_db_locked_hint(&e) {
                eprintln!("[error] database is locked: {hint}");
                eprintln!(
                    "  another codenexus process may be writing (index/import hold an exclusive DB lock)"
                );
                eprintln!(
                    "  close other codenexus processes and retry, or wait for them to finish"
                );
                std::process::exit(2);
            }
            eprintln!("[warn] Failed to build Kit (commands requiring Kit will fail): {e}");
        }
    }

    let args = extract_args(sub_name, sub_matches, &project_config);

    let handler = match sdforge::inventory::iter::<sdforge::cli::CliHandlerRegistration>()
        .find(|h| h.name == sub_name)
    {
        Some(h) => h,
        None => {
            eprintln!("[error] No handler registered for command: {sub_name}");
            std::process::exit(1);
        }
    };

    if let Err(api_error) = runtime.block_on((handler.handler)(args, None)) {
        let cli_error = CodeNexusError::from(api_error);
        eprintln!("Error: {cli_error}");
        std::process::exit(cli_error.exit_code());
    }

    // Graceful trait-kit module shutdown: drains the lifecycle `on_shutdown`
    // hooks (cache/storage close diagnostics) in reverse topological order.
    // Best-effort — one-shot CLI exits right after this.
    if let Some(kit) = codenexus::service::runtime::kit() {
        runtime.block_on(kit.shutdown_async());
    }
}

/// Returns the value of the global arg `name` when it was explicitly
/// provided on the command line (not a clap `default_value`), checking
/// top-level then subcommand matches. `None` means "caller falls back to
/// derived / config-file / built-in defaults".
fn explicit_cli_arg(
    top: &sdforge::clap::ArgMatches,
    sub: &sdforge::clap::ArgMatches,
    name: &str,
) -> Option<String> {
    let from_cli = |m: &sdforge::clap::ArgMatches| {
        m.value_source(name) == Some(sdforge::clap::parser::ValueSource::CommandLine)
    };
    if from_cli(top) {
        return top.get_one::<String>(name).cloned();
    }
    if from_cli(sub) {
        return sub.get_one::<String>(name).cloned();
    }
    None
}

/// Computes the default database path when `--db` is not specified.
///
/// Resolves to `.codenexus/<project>.lbug`, where `<project>` is derived from
/// the subcommand's `name` arg when present, otherwise the directory name of
/// its `path` arg. When neither is available (read-only commands like
/// `query`/`list`/`search`/`impact`/`context`), the `.codenexus/` directory
/// is scanned for an existing `.lbug` file:
///
/// - Exactly one file → use it (common case: single indexed project in CWD).
/// - Zero or multiple files → fall back to [`FALLBACK_PROJECT_NAME`], which
///   `validate_db_exists` will reject with a clear "run `codenexus index`"
///   message (failures must surface, not silently succeed).
///
/// This fixes the bulwark regression where `query`/`list` without `--db`
/// returned exit 4 NotFound even though `.codenexus/bulwark.lbug` existed —
/// the old code unconditionally fell back to `codenexus` (this repo's own
/// name), producing `.codenexus/codenexus.lbug` which does not exist in
/// downstream project workspaces.
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
        });

    let project = match raw_project {
        Some(name) => sanitize_project_name(&name),
        None => match discover_single_indexed_db() {
            Some(path) => return path,
            None => FALLBACK_PROJECT_NAME.to_string(),
        },
    };
    format!("{DEFAULT_DB_DIR}/{project}.lbug")
}

/// Scans the `.codenexus/` directory in the current working directory for an
/// existing `.lbug` database file. Returns the full path when exactly one is
/// found (the unambiguous case); returns `None` for zero or multiple files.
///
/// Used by [`default_db_path`] to pick up an already-indexed project when the
/// user runs a read-only command (`query`, `list`, `search`, ...) without
/// `--db` in the project's own workspace.
fn discover_single_indexed_db() -> Option<String> {
    let dir = std::path::Path::new(DEFAULT_DB_DIR);
    let entries = std::fs::read_dir(dir).ok()?;
    let mut lbug_files: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            let ext = path.extension().and_then(|x| x.to_str())?;
            if ext == "lbug" {
                path.to_str().map(String::from)
            } else {
                None
            }
        })
        .collect();
    match lbug_files.len() {
        1 => lbug_files.pop(),
        _ => None,
    }
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

/// Commands that require an existing database file. `index`/`import` create
/// one (LadybugDB create-on-open); `setup`/`lsp_*`/`hook`/`mcp` don't query
/// the graph. Without this gate, `list --db <missing>` silently builds an
/// empty DB and returns `[]` with exit 0 — a silent success.
///
/// Includes both read-only commands (see [`opens_read_only`]) and read-write
/// commands that operate on an existing DB (`clean`, `rename`, `daemon`,
/// `detect_changes`, `export`).
fn requires_existing_db(sub_name: &str) -> bool {
    opens_read_only(sub_name)
        || matches!(
            sub_name,
            "clean" | "rename" | "daemon" | "detect_changes" | "export"
        )
}

/// Read-only commands that open the DB in shared-read mode (concurrent-safe).
/// These commands never write to the DB, so they can share a read lock with
/// other processes. Writing commands (`index`/`import`/`clean`/`rename`/
/// `daemon`) keep read-write mode.
fn opens_read_only(sub_name: &str) -> bool {
    matches!(
        sub_name,
        "list"
            | "status"
            | "search"
            | "query"
            | "impact"
            | "context"
            | "trace"
            | "dead_code"
            | "community"
            | "architecture"
            | "complexity"
            | "diagram"
            | "arch_diff"
            | "cross_service"
            | "route_map"
            | "tool_map"
            | "shape_check"
            | "api_impact"
            | "ci"
            | "ask"
            | "lint"
            | "taint"
            | "supply"
    )
}

/// Walks the [`KitError`] source chain for [`StorageError::DatabaseLocked`]
/// Returns the holder hint so the CLI can exit 2 with a clear
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

/// Deletes the DB file for `--fresh` mode (space reclamation).
///
/// Returns `Ok(true)` if the file was deleted, `Ok(false)` if it did not
/// exist (not an error — `--fresh` only ensures a clean state), or `Err` on
/// failure (including refusal to delete non-`.lbug` files).
///
/// # Safety
///
/// Refuses to delete files without the `.lbug` extension. Without this,
/// `codenexus index --db /home/user/important.txt --fresh true` would delete
/// an arbitrary file — violating least-surprise (`--db` semantically points
/// at a database, not any file).
///
/// # TOCTOU
///
/// Calls `remove_file` directly without a prior `exists()` check, matching
/// `ErrorKind::NotFound` to handle the "already absent" case. This eliminates
/// the time-of-check-to-time-of-use window between `exists()` and
/// `remove_file()`.
fn delete_db_for_fresh(db: &str) -> std::io::Result<bool> {
    let db_path = PathBuf::from(db);
    if db_path.extension().and_then(|s| s.to_str()) != Some("lbug") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("refusing to delete non-.lbug file: {}", db_path.display()),
        ));
    }
    match std::fs::remove_file(&db_path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Handles the `--fresh` CLI flag: deletes the existing DB file before Kit
/// init so Kit opens a fresh one (DuckDB DELETE doesn't reclaim pages and
/// LadybugDB doesn't support VACUUM, so repeated `--force` indexes accumulate
/// dead space).
///
/// Only the `index` command supports `--fresh` (the `import` command has no
/// `fresh` parameter in its `#[forge]` signature, so sdforge does not
/// register it; `try_get_one` returns `Err` → no-op). Other commands would
/// lose data without re-indexing.
///
/// Exits with code 6 if `--db` points at a non-`.lbug` file (safety), or code
/// 5 if deletion fails (failure must surface, not be swallowed).
/// Code 4 is reserved for NotFound (validate_db_exists).
fn handle_fresh_flag(sub_name: &str, sub_matches: &sdforge::clap::ArgMatches, db: &str) {
    if sub_name != "index" {
        return;
    }
    let fresh = sub_matches
        .try_get_one::<String>("fresh")
        .ok()
        .flatten()
        .is_some_and(|s| s.parse::<bool>().unwrap_or(false));
    if !fresh {
        return;
    }
    match delete_db_for_fresh(db) {
        Ok(true) => {
            eprintln!("[info] --fresh: deleted existing DB file {}", db);
        }
        Ok(false) => {
            eprintln!("[info] --fresh: no existing DB file at {db}, nothing to delete");
        }
        Err(e) => {
            eprintln!("[error] --fresh: {e}");
            std::process::exit(if e.kind() == std::io::ErrorKind::InvalidInput {
                6
            } else {
                5
            });
        }
    }
}

/// Handles the `clean --rebuild` flag: deletes the whole DB file (plus its
/// WAL sidecar) before Kit init and exits. Unlike the normal `clean` (project
/// removal via the graph), rebuild is the self-healing path for a database
/// that can no longer be opened usefully — schema-version mismatch (exit 4
/// from the storage layer's version check) or corruption. Deleting the file
/// must happen before Kit opens it, so this hook lives in `run_cli`.
///
/// Exits 0 after deletion (the caller re-runs `codenexus index` to rebuild).
/// A missing DB file is a no-op success. The `.lbug`-extension refusal from
/// [`delete_db_for_fresh`] applies.
fn handle_clean_rebuild(sub_name: &str, sub_matches: &sdforge::clap::ArgMatches, db: &str) {
    if sub_name != "clean" {
        return;
    }
    let rebuild = sub_matches
        .try_get_one::<String>("rebuild")
        .ok()
        .flatten()
        .is_some_and(|s| s.parse::<bool>().unwrap_or(false));
    if !rebuild {
        return;
    }
    match delete_db_for_fresh(db) {
        Ok(true) => {
            let _ = std::fs::remove_file(format!("{db}.wal"));
            println!("[info] --rebuild: deleted database file {db}");
            println!("  run `codenexus index ...` to rebuild the knowledge graph");
            std::process::exit(0);
        }
        Ok(false) => {
            println!("[info] --rebuild: no database file at {db}, nothing to reset");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("[error] --rebuild: {e}");
            std::process::exit(if e.kind() == std::io::ErrorKind::InvalidInput {
                6
            } else {
                5
            });
        }
    }
}

/// Extracts subcommand args into a `HashMap<String, String>` for the handler.
///
/// Per-value precedence: explicit CLI flag > `command_defaults` entry in
/// `.codenexus/config.json` > clap default (the sentinel defaults applied by
/// [`apply_sentinel_defaults`]). Config values only apply to args registered
/// on that subcommand — unknown keys in the config are inert here.
fn extract_args(
    sub_name: &str,
    sub_matches: &sdforge::clap::ArgMatches,
    config: &ProjectConfig,
) -> HashMap<String, String> {
    let from_cli = |m: &sdforge::clap::ArgMatches, name: &str| {
        m.value_source(name) == Some(sdforge::clap::parser::ValueSource::CommandLine)
    };
    let mut args = HashMap::new();
    for reg in sdforge::inventory::iter::<sdforge::cli::CliCommandRegistration>() {
        if reg.name == sub_name {
            for arg_info in reg.args {
                let cli_value = sub_matches.get_one::<String>(arg_info.name).cloned();
                if from_cli(sub_matches, arg_info.name) {
                    if let Some(value) = cli_value {
                        args.insert(arg_info.name.to_string(), value);
                    }
                } else if let Some(value) = config.command_default(sub_name, arg_info.name) {
                    args.insert(arg_info.name.to_string(), value);
                } else if let Some(value) = cli_value {
                    args.insert(arg_info.name.to_string(), value);
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
/// A `.codenexus/config.json` `command_defaults` entry takes precedence over
/// these sentinel defaults (see [`ProjectConfig`] and [`extract_args`]).
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
    // — diagram: quality/repo_root/repo_url/title/locale default at the
    // service layer; empty repo_root skips evidence verification.
    // viewer_data/viewer_url default empty (viewer mode off).
    ("diagram", "quality", "standard"),
    ("diagram", "repo_root", ""),
    ("diagram", "repo_url", ""),
    ("diagram", "title", ""),
    ("diagram", "locale", "en"),
    ("diagram", "viewer_data", ""),
    ("diagram", "viewer_url", ""),
    // — arch_diff: quality defaults to standard, title defaults to the
    // generated "<base> → <head>" caption; viewer params default off.
    ("arch_diff", "quality", "standard"),
    ("arch_diff", "title", ""),
    ("arch_diff", "viewer_data", ""),
    ("arch_diff", "viewer_url", ""),
    // — dead_code: check_dynamic_dispatch defaults to true so trait
    // impl methods (e.g. `fmt#Display`) are excluded by default. Users can
    // opt out via `--check_dynamic_dispatch false` for adversarial testing.
    ("dead_code", "check_dynamic_dispatch", "true"),
    // — dead_code: reflection/derive-macro entry points default off
    // (sentinel missed when the param was introduced — exit-2 regression).
    ("dead_code", "check_reflection", "false"),
    // — index: fresh defaults to false so users can omit --fresh; only
    // --fresh true triggers DB file deletion (P-DB space reclamation).
    ("index", "fresh", "false"),
    // — clean: rebuild defaults to false; --rebuild true deletes the whole
    // DB file (self-healing for schema-version mismatch or corruption) and
    // is intercepted before Kit init in run_cli.
    ("clean", "rebuild", "false"),
    // — index: pipeline toggles default to off so users can index with just
    // --path/--name; per-project settings can pin them via
    // .codenexus/config.json command_defaults (extract_args overlay).
    ("index", "force", "false"),
    ("index", "lsp", "false"),
    ("index", "embed", "false"),
    ("index", "ram_first", "false"),
    // — detect_changes: mode defaults to unstaged (the common pre-commit
    // case); path stays required (no sensible default repo root).
    ("detect_changes", "mode", "unstaged"),
    // — trace: depth defaults to 5 (the engine rejects 0 with
    // TraceError::InvalidDepth, and the USER_GUIDE quickstart uses 5);
    // path_filter/cycle/cross-service flags default to no-filter/no-scan.
    // `trace_type` stays required — calls|dataflow|all is a genuine choice.
    ("trace", "depth", "5"),
    ("trace", "path_filter", ""),
    ("trace", "detect_cycles", "false"),
    ("trace", "cross_service", "false"),
    // — search: limit defaults to 50 (crate DEFAULT_LIMIT; a bare 0 would
    // truncate results to empty); empty mode = legacy path, empty project =
    // all projects.
    ("search", "fulltext", "false"),
    ("search", "limit", "50"),
    ("search", "mode", ""),
    ("search", "project", ""),
    // — impact: legacy depth defaults to 3 (USER_GUIDE example; 0 would load
    // only the start node); empty edge_types / max_depth 0 / include_tests
    // false select the legacy single-dimension path (0 sentinel = "use
    // built-in default 5" in the enhanced path).
    ("impact", "depth", "3"),
    ("impact", "edge_types", ""),
    ("impact", "max_depth", "0"),
    ("impact", "include_tests", "false"),
    // — context: depth defaults to 1 (USER_GUIDE example), empty project =
    // no project validation, enhanced=false = legacy 360 view.
    // NOTE: no `budget` entry — the `context` CLI command does not define a
    // `budget` parameter (token budgeting lives in service::budget, applied
    // internally); a sentinel entry here would panic clap's `mut_arg`
    // ("Argument `budget` is undefined") on every CLI startup.
    ("context", "depth", "1"),
    ("context", "project", ""),
    ("context", "enhanced", "false"),
    // — ci: base_mode defaults to head (CI diffs committed work against
    // HEAD); fail_on defaults to high (only the biggest blast radii block);
    // empty project = no project validation.
    ("ci", "base_mode", "head"),
    ("ci", "fail_on", "high"),
    ("ci", "project", ""),
    // — skill: target defaults to auto (all detected agents).
    ("skill", "target", "auto"),
    // — ask: execute defaults to true (run the matched command in-process);
    // empty project = project-scoped commands use the default project.
    ("ask", "execute", "true"),
    ("ask", "project", ""),
    // — lint: rules file defaults to .codenexus/rules.json; fail_on defaults
    // to error (only error-severity violations block).
    ("lint", "rules", ".codenexus/rules.json"),
    ("lint", "fail_on", "error"),
    ("lint", "project", ""),
    // NOTE: no `daemon notify_impact` entry — the `daemon` CLI command only
    // defines `path`/`name`; impact notifications have no CLI flag today
    // (a sentinel entry here would panic clap's `mut_arg`
    // ("Argument `notify_impact` is undefined") on every CLI startup).
    // — taint: rules mode by default (empty source/sink); language filter
    // empty = all rule languages.
    ("taint", "source", ""),
    ("taint", "sink", ""),
    ("taint", "language", ""),
    ("taint", "max_pairs", "200"),
    ("taint", "depth", "8"),
    ("taint", "project", ""),
    // — supply: empty project = no project validation.
    ("supply", "project", ""),
    // — evolve: output dir default; max_commits default 10 (cap 50 enforced
    // in the service); empty project = directory base name.
    ("evolve", "output", ".codenexus/evolve"),
    ("evolve", "max_commits", "10"),
    ("evolve", "project", ""),
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
///
/// The Kit opens the DB **read-only**: the MCP tool surface is query-only by
/// design (query/trace/impact/search/context/architecture/diagram/arch_diff —
/// write tools like index/import/rename are CLI-only), so a shared-read open
/// lets the server run alongside an active `index`/`daemon` writer instead of
/// either side failing with `DatabaseLocked` (LadybugDB is single-writer).
#[cfg(feature = "mcp")]
fn run_mcp_server() {
    let db = parse_mcp_db_arg().unwrap_or_else(|| {
        // Same fallback chain as read-only CLI commands: pick up the single
        // indexed project in .codenexus/ before falling back to the default
        // name (discover_single_indexed_db returns None for 0 or 2+ files).
        discover_single_indexed_db()
            .unwrap_or_else(|| format!("{DEFAULT_DB_DIR}/{FALLBACK_PROJECT_NAME}.lbug"))
    });

    // Read-only open does not create-on-open, so a missing DB must be
    // rejected up front with the same message shape as read-only CLI
    // commands (validate_db_exists) and exit code 4 (NotFound).
    if !PathBuf::from(&db).exists() {
        eprintln!(
            "[error] database not found: {db}\n  the MCP server opens the DB read-only; run `codenexus index ...` first, or pass --db <path>"
        );
        std::process::exit(4);
    }

    // Create the tokio runtime early — build_kit is async (AsyncKit migration)
    // and the same runtime is reused for MCP serve below.
    let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    let config = KitBootstrapConfig::new(PathBuf::from(&db)).with_read_only(true);
    let kit = match runtime.block_on(build_kit(&config)) {
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

/// Parses the MCP mode arguments, accepting both `--db <path>` and
/// `--db=<path>`. Unknown arguments are rejected with exit 2 instead of being
/// silently ignored — the old space-only parser turned `--db=/x/y.lbug` into
/// a silent fallback to the default DB (queries then returned empty results
/// with no hint why). Returns `None` when no `--db` was given.
#[cfg(feature = "mcp")]
fn parse_mcp_db_arg() -> Option<String> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    match args.first().map(String::as_str) {
        None => None,
        Some("--db") => match args.get(1) {
            Some(v) => Some(v.clone()),
            None => {
                eprintln!("[error] --db requires a value");
                eprintln!("  Usage: codenexus mcp [--db <path>]");
                std::process::exit(2);
            }
        },
        Some(arg) => {
            if let Some(value) = arg.strip_prefix("--db=") {
                return Some(value.to_string());
            }
            if arg == "-h" || arg == "--help" {
                println!("Usage: codenexus mcp [--db <path>]");
                println!();
                println!("Serves the CodeNexus MCP server over stdio (read-only DB access).");
                std::process::exit(0);
            }
            eprintln!("[error] unrecognized argument for `codenexus mcp`: {arg}");
            eprintln!("  Usage: codenexus mcp [--db <path>]");
            std::process::exit(2);
        }
    }
}

/// Parsed contents of `.codenexus/config.json` — the per-project config
/// file. Precedence everywhere it is consulted: explicit CLI flag > config
/// file > built-in default.
///
/// Schema (both sections optional, unknown keys warn but do not fail):
///
/// ```json
/// {
///   "general": {
///     "db": ".codenexus/team.lbug",
///     "debounce_ms": 2000,
///     "verbose": false
///   },
///   "command_defaults": {
///     "index": { "ram_first": true },
///     "complexity": { "cyclomatic_red": 25 }
///   }
/// }
/// ```
///
/// `command_defaults` maps a subcommand name to `{arg: value}` entries; the
/// arg must be registered on that subcommand and values are stringified
/// (bools → "true"/"false", numbers → decimal text) before being handed to
/// the handler. Config defaults only take effect where the user did not pass
/// the flag explicitly; they override the sentinel defaults.
#[derive(Debug, Default, PartialEq)]
struct ProjectConfig {
    db: Option<String>,
    debounce_ms: Option<u64>,
    notify_webhook: Option<String>,
    verbose: Option<bool>,
    command_defaults: HashMap<String, HashMap<String, String>>,
}

impl ProjectConfig {
    /// Parses a `serde_json::Value` into a [`ProjectConfig`], collecting
    /// non-fatal shape warnings (unknown keys / wrong value types) instead of
    /// failing — a broken config must not take down every command.
    fn from_json(value: &serde_json::Value) -> (Self, Vec<String>) {
        let mut cfg = Self::default();
        let mut warnings = Vec::new();
        let Some(root) = value.as_object() else {
            warnings.push("root is not a JSON object; ignoring file".to_string());
            return (cfg, warnings);
        };

        match root.get("general") {
            Some(serde_json::Value::Object(g)) => {
                cfg.db = Self::string_field(g, "db", &mut warnings);
                cfg.debounce_ms = Self::u64_field(g, "debounce_ms", &mut warnings);
                cfg.notify_webhook = Self::string_field(g, "notify_webhook", &mut warnings);
                cfg.verbose = Self::bool_field(g, "verbose", &mut warnings);
                for key in g.keys() {
                    if !matches!(
                        key.as_str(),
                        "db" | "debounce_ms" | "notify_webhook" | "verbose"
                    ) {
                        warnings.push(format!(
                            "general.{key}: unknown key (expected db / debounce_ms / notify_webhook / verbose)"
                        ));
                    }
                }
            }
            Some(_) => warnings.push("general: not a JSON object; section ignored".to_string()),
            None => {}
        }

        match root.get("command_defaults") {
            Some(serde_json::Value::Object(commands)) => {
                for (command, args) in commands {
                    let Some(args_obj) = args.as_object() else {
                        warnings.push(format!(
                            "command_defaults.{command}: not a JSON object; entry ignored"
                        ));
                        continue;
                    };
                    let entry = cfg.command_defaults.entry(command.clone()).or_default();
                    for (arg, value) in args_obj {
                        match Self::scalar_to_string(value) {
                            Some(s) => {
                                entry.insert(arg.clone(), s);
                            }
                            None => warnings.push(format!(
                                "command_defaults.{command}.{arg}: value must be a string, number, or boolean"
                            )),
                        }
                    }
                }
            }
            Some(_) => {
                warnings.push("command_defaults: not a JSON object; section ignored".to_string())
            }
            None => {}
        }

        for key in root.keys() {
            if !matches!(key.as_str(), "general" | "command_defaults") {
                warnings.push(format!(
                    "{key}: unknown section (expected general / command_defaults)"
                ));
            }
        }

        (cfg, warnings)
    }

    /// String parsing entry point; `Err` carries the serde_json message.
    fn from_str(json: &str) -> Result<(Self, Vec<String>), String> {
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|e| format!("invalid JSON: {e}"))?;
        Ok(Self::from_json(&value))
    }

    /// Loads `.codenexus/config.json` relative to the CWD. A missing file is
    /// the normal case (default config, no warnings); unreadable or invalid
    /// JSON degrades to the default config plus one warning.
    fn load() -> (Self, Vec<String>) {
        let path = format!("{DEFAULT_DB_DIR}/config.json");
        match std::fs::read_to_string(&path) {
            Ok(raw) => match Self::from_str(&raw) {
                Ok(parsed) => parsed,
                Err(e) => (Self::default(), vec![format!("{e}; ignoring file")]),
            },
            Err(_) => (Self::default(), Vec::new()),
        }
    }

    /// Returns the configured default for `arg` of `command`, if any.
    fn command_default(&self, command: &str, arg: &str) -> Option<String> {
        self.command_defaults.get(command)?.get(arg).cloned()
    }

    fn string_field(
        obj: &serde_json::Map<String, serde_json::Value>,
        key: &str,
        warnings: &mut Vec<String>,
    ) -> Option<String> {
        match obj.get(key) {
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(_) => {
                warnings.push(format!("general.{key}: expected a string; ignored"));
                None
            }
            None => None,
        }
    }

    fn u64_field(
        obj: &serde_json::Map<String, serde_json::Value>,
        key: &str,
        warnings: &mut Vec<String>,
    ) -> Option<u64> {
        match obj.get(key) {
            Some(serde_json::Value::Number(n)) => n.as_u64().or_else(|| {
                warnings.push(format!(
                    "general.{key}: expected a non-negative integer; ignored"
                ));
                None
            }),
            Some(_) => {
                warnings.push(format!("general.{key}: expected a number; ignored"));
                None
            }
            None => None,
        }
    }

    fn bool_field(
        obj: &serde_json::Map<String, serde_json::Value>,
        key: &str,
        warnings: &mut Vec<String>,
    ) -> Option<bool> {
        match obj.get(key) {
            Some(serde_json::Value::Bool(b)) => Some(*b),
            Some(_) => {
                warnings.push(format!("general.{key}: expected a boolean; ignored"));
                None
            }
            None => None,
        }
    }

    fn scalar_to_string(value: &serde_json::Value) -> Option<String> {
        match value {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Bool(b) => Some(b.to_string()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    }
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

    // --- explicit_cli_arg ---

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
    fn explicit_cli_arg_returns_top_value_when_present() {
        let cmd = build_test_cmd();
        let matches = cmd.get_matches_from(["codenexus", "--db", "/top/db", "index"]);
        let sub = matches.subcommand_matches("index").unwrap();
        assert_eq!(
            explicit_cli_arg(&matches, sub, "db").as_deref(),
            Some("/top/db")
        );
    }

    #[test]
    fn explicit_cli_arg_returns_sub_value_when_top_absent() {
        let cmd = sdforge::clap::Command::new("codenexus")
            .arg(sdforge::clap::Arg::new("db").long("db").global(true))
            .subcommand(
                sdforge::clap::Command::new("index").arg(sdforge::clap::Arg::new("db").long("db")),
            );
        let matches = cmd.get_matches_from(["codenexus", "index", "--db", "/sub/db"]);
        let sub = matches.subcommand_matches("index").unwrap();
        assert_eq!(
            explicit_cli_arg(&matches, sub, "db").as_deref(),
            Some("/sub/db")
        );
    }

    #[test]
    fn explicit_cli_arg_returns_none_for_defaults_and_absent_args() {
        // `db` has a default_value in build_test_cmd — present but not
        // explicit, so the caller must fall back to derived/config defaults.
        let cmd = build_test_cmd();
        let matches = cmd.get_matches_from(["codenexus", "index"]);
        let sub = matches.subcommand_matches("index").unwrap();
        assert_eq!(explicit_cli_arg(&matches, sub, "db"), None);

        // `unused` is undefined on `index` — value_source errors must be
        // swallowed (try-style), never panic.
        let cmd2 = sdforge::clap::Command::new("codenexus")
            .arg(
                sdforge::clap::Arg::new("unused")
                    .long("unused")
                    .global(true),
            )
            .subcommand(sdforge::clap::Command::new("index"));
        let matches2 = cmd2.get_matches_from(["codenexus", "index"]);
        let sub2 = matches2.subcommand_matches("index").unwrap();
        assert_eq!(explicit_cli_arg(&matches2, sub2, "unused"), None);
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
    #[serial_test::serial(db_discover)]
    fn default_db_path_falls_back_when_no_args() {
        let cmd = index_sub_with(None, None);
        let m = cmd.get_matches_from(["codenexus", "index"]);
        let sub = m.subcommand_matches("index").unwrap();
        assert_eq!(default_db_path(sub), ".codenexus/codenexus.lbug");
    }

    #[test]
    #[serial_test::serial(db_discover)]
    fn default_db_path_does_not_panic_when_subcommand_has_no_name_arg() {
        // Real commands like `search`/`impact`/`context` do not define a `name`
        // or `path` arg. default_db_path must use [`FALLBACK_PROJECT_NAME`]
        // (after [`discover_single_indexed_db`] returns `None`) instead of
        // panicking. Regression: clap `get_one` panics on an arg id that the
        // Command never defined.
        let sub_cmd = sdforge::clap::Command::new("search");
        let cmd = sdforge::clap::Command::new("codenexus").subcommand(sub_cmd);
        let m = cmd.get_matches_from(["codenexus", "search"]);
        let sub = m.subcommand_matches("search").unwrap();
        assert_eq!(default_db_path(sub), ".codenexus/codenexus.lbug");
    }

    // --- requires_existing_db / opens_read_only / validate_db_exists ---

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
    fn validate_db_exists_errors_for_status_when_db_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("nope.lbug");
        let err = validate_db_exists(missing.to_str().unwrap(), "status").unwrap_err();
        assert!(err.contains("database not found"), "got: {err}");
    }

    #[test]
    fn validate_db_exists_errors_for_analysis_cmds_when_db_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("nope.lbug");
        for cmd in [
            "dead_code",
            "community",
            "architecture",
            "complexity",
            "diagram",
            "arch_diff",
            "cross_service",
            "route_map",
            "tool_map",
            "shape_check",
            "api_impact",
        ] {
            let err = validate_db_exists(missing.to_str().unwrap(), cmd).unwrap_err();
            assert!(err.contains("database not found"), "{cmd}: got {err}");
        }
    }

    #[test]
    fn validate_db_exists_errors_for_clean_rename_daemon_when_db_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("nope.lbug");
        for cmd in ["clean", "rename", "daemon", "detect_changes", "export"] {
            let err = validate_db_exists(missing.to_str().unwrap(), cmd).unwrap_err();
            assert!(err.contains("database not found"), "{cmd}: got {err}");
        }
    }

    #[test]
    fn validate_db_exists_allows_import_to_create_new_db() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("new.lbug");
        assert!(validate_db_exists(missing.to_str().unwrap(), "import").is_ok());
    }

    #[test]
    fn validate_db_exists_allows_setup_hook_lsp_mcp_without_db() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("nope.lbug");
        for cmd in ["setup", "hook", "lsp_hover", "lsp_goto_def", "mcp"] {
            assert!(
                validate_db_exists(missing.to_str().unwrap(), cmd).is_ok(),
                "{cmd} should not require existing DB"
            );
        }
    }

    // --- delete_db_for_fresh ---

    #[test]
    fn delete_db_for_fresh_deletes_existing_lbug_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = dir.path().join("proj.lbug");
        std::fs::write(&db, b"dummy").unwrap();
        let result = delete_db_for_fresh(db.to_str().unwrap());
        assert!(result.is_ok(), "expected Ok, got {:?}", result.err());
        assert!(result.unwrap(), "should report deleted");
        assert!(!db.exists(), "file should be deleted");
    }

    #[test]
    fn delete_db_for_fresh_returns_ok_false_when_file_not_exists() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = dir.path().join("never_existed.lbug");
        let result = delete_db_for_fresh(db.to_str().unwrap());
        assert!(result.is_ok(), "expected Ok, got {:?}", result.err());
        assert!(!result.unwrap(), "should report not-existed");
    }

    #[test]
    fn delete_db_for_fresh_refuses_non_lbug_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = dir.path().join("important.txt");
        std::fs::write(&db, b"do not delete").unwrap();
        let result = delete_db_for_fresh(db.to_str().unwrap());
        let err = result.expect_err("non-.lbug should be refused");
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::InvalidInput,
            "expected InvalidInput, got {err:?}"
        );
        assert!(db.exists(), "non-.lbug file must not be deleted");
    }

    #[test]
    fn delete_db_for_fresh_returns_err_for_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        // Name with .lbug extension but actually a directory — remove_file
        // fails on directories with PermissionDenied or IsADirectory.
        let fake_db = dir.path().join("dir.lbug");
        std::fs::create_dir(&fake_db).unwrap();
        let result = delete_db_for_fresh(fake_db.to_str().unwrap());
        assert!(result.is_err(), "directory should cause error, got Ok");
    }

    #[test]
    fn opens_read_only_returns_true_for_read_only_commands() {
        for cmd in [
            "list",
            "status",
            "search",
            "query",
            "impact",
            "context",
            "trace",
            "dead_code",
            "community",
            "architecture",
            "complexity",
            "diagram",
            "arch_diff",
            "cross_service",
            "route_map",
            "tool_map",
            "shape_check",
            "api_impact",
        ] {
            assert!(opens_read_only(cmd), "{cmd} should be read-only");
        }
    }

    #[test]
    fn opens_read_only_returns_false_for_writing_commands() {
        for cmd in ["index", "import", "clean", "rename", "daemon"] {
            assert!(!opens_read_only(cmd), "{cmd} should NOT be read-only");
        }
    }

    #[test]
    fn opens_read_only_returns_false_for_non_db_commands() {
        for cmd in ["setup", "hook", "lsp_hover", "lsp_goto_def", "mcp"] {
            assert!(!opens_read_only(cmd), "{cmd} should NOT be read-only");
        }
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
        let args = extract_args("nonexistent_cmd", sub, &ProjectConfig::default());
        assert!(
            args.is_empty(),
            "unregistered command should return empty args"
        );
    }

    // --- discover_single_indexed_db (M3: multi-`.lbug` fallback coverage) ---

    /// Restores the process cwd on drop. Used by the
    /// `discover_single_indexed_db_*` tests below so the original cwd is
    /// restored even if an assertion panics after a `set_current_dir`.
    struct CwdGuard {
        original: std::path::PathBuf,
    }

    impl CwdGuard {
        fn enter(new_dir: &std::path::Path) -> Self {
            let original = std::env::current_dir().expect("failed to read cwd");
            std::env::set_current_dir(new_dir).expect("failed to chdir into temp");
            CwdGuard { original }
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.original);
        }
    }

    /// `discover_single_indexed_db` returns `None` when `.codenexus/`
    /// contains multiple `.lbug` files, forcing the caller to fall back
    /// to [`FALLBACK_PROJECT_NAME`]. This test creates a temporary
    /// working directory with `.codenexus/` holding two `.lbug` files,
    /// chdirs into it, and verifies the function returns `None`.
    ///
    /// Serialized via `serial_test::serial(db_discover)` because the
    /// function reads the relative `.codenexus/` path from the process
    /// cwd — concurrent chdir would race with other cwd-dependent tests.
    #[test]
    #[serial_test::serial(db_discover)]
    fn discover_single_indexed_db_returns_none_when_multiple_lbug_files() {
        let tmp = tempfile::TempDir::new().expect("failed to create temp dir");
        let _guard = CwdGuard::enter(tmp.path());

        let codenexus_dir = tmp.path().join(DEFAULT_DB_DIR);
        std::fs::create_dir_all(&codenexus_dir).expect("failed to create .codenexus/");
        std::fs::write(codenexus_dir.join("project_a.lbug"), b"a").expect("write a.lbug");
        std::fs::write(codenexus_dir.join("project_b.lbug"), b"b").expect("write b.lbug");

        let result = discover_single_indexed_db();
        assert!(
            result.is_none(),
            "multiple .lbug files must return None (got {:?})",
            result
        );
    }

    /// `discover_single_indexed_db` returns `Some(path)` when `.codenexus/`
    /// contains exactly one `.lbug` file. Companion to the multi-file case
    /// above; ensures the single-file happy path is also covered.
    #[test]
    #[serial_test::serial(db_discover)]
    fn discover_single_indexed_db_returns_path_when_single_lbug_file() {
        let tmp = tempfile::TempDir::new().expect("failed to create temp dir");
        let _guard = CwdGuard::enter(tmp.path());

        let codenexus_dir = tmp.path().join(DEFAULT_DB_DIR);
        std::fs::create_dir_all(&codenexus_dir).expect("failed to create .codenexus/");
        std::fs::write(codenexus_dir.join("only_project.lbug"), b"only")
            .expect("write only_project.lbug");

        let result = discover_single_indexed_db();
        // `discover_single_indexed_db` reads the relative `.codenexus/`
        // path, so the returned string is relative (e.g.
        // `.codenexus/only_project.lbug`). Assert the file name matches
        // rather than the full path to stay robust against cwd.
        let returned = result.expect("single .lbug file must return Some(path)");
        assert!(
            returned.ends_with("only_project.lbug"),
            "expected path ending with only_project.lbug, got {returned:?}"
        );
    }

    /// `discover_single_indexed_db` returns `None` when `.codenexus/`
    /// does not exist (zero-file case). Ensures the missing-directory
    /// path is exercised and does not panic.
    #[test]
    #[serial_test::serial(db_discover)]
    fn discover_single_indexed_db_returns_none_when_directory_missing() {
        let tmp = tempfile::TempDir::new().expect("failed to create temp dir");
        let _guard = CwdGuard::enter(tmp.path());
        // Intentionally do NOT create .codenexus/.
        let result = discover_single_indexed_db();
        assert!(
            result.is_none(),
            "missing .codenexus/ must return None (got {:?})",
            result
        );
    }

    // --- build_cli_command: sentinel defaults for high-frequency commands ---

    #[test]
    fn sentinel_defaults_relax_high_frequency_commands() {
        let cmd = build_cli_command();
        let m = cmd.get_matches_from([
            "codenexus",
            "trace",
            "--symbol",
            "main",
            "--trace_type",
            "calls",
        ]);
        let sub = m.subcommand_matches("trace").unwrap();
        assert_eq!(
            sub.get_one::<String>("depth").map(String::as_str),
            Some("5")
        );
        assert_eq!(
            sub.get_one::<String>("path_filter").map(String::as_str),
            Some("")
        );
        assert_eq!(
            sub.get_one::<String>("detect_cycles").map(String::as_str),
            Some("false")
        );
        assert_eq!(
            sub.get_one::<String>("cross_service").map(String::as_str),
            Some("false")
        );

        let cmd2 = build_cli_command();
        let m2 = cmd2.get_matches_from(["codenexus", "search", "--text", "parse"]);
        let sub2 = m2.subcommand_matches("search").unwrap();
        assert_eq!(
            sub2.get_one::<String>("limit").map(String::as_str),
            Some("50")
        );
        assert_eq!(sub2.get_one::<String>("mode").map(String::as_str), Some(""));
        assert_eq!(
            sub2.get_one::<String>("project").map(String::as_str),
            Some("")
        );
        assert_eq!(
            sub2.get_one::<String>("fulltext").map(String::as_str),
            Some("false")
        );

        let cmd3 = build_cli_command();
        let m3 = cmd3.get_matches_from(["codenexus", "impact", "--symbol", "main"]);
        let sub3 = m3.subcommand_matches("impact").unwrap();
        assert_eq!(
            sub3.get_one::<String>("depth").map(String::as_str),
            Some("3")
        );
        assert_eq!(
            sub3.get_one::<String>("max_depth").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            sub3.get_one::<String>("edge_types").map(String::as_str),
            Some("")
        );
        assert_eq!(
            sub3.get_one::<String>("include_tests").map(String::as_str),
            Some("false")
        );

        let cmd4 = build_cli_command();
        let m4 = cmd4.get_matches_from(["codenexus", "context", "--symbol", "main"]);
        let sub4 = m4.subcommand_matches("context").unwrap();
        assert_eq!(
            sub4.get_one::<String>("depth").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            sub4.get_one::<String>("project").map(String::as_str),
            Some("")
        );
        assert_eq!(
            sub4.get_one::<String>("enhanced").map(String::as_str),
            Some("false")
        );

        let cmd5 = build_cli_command();
        let m5 =
            cmd5.get_matches_from(["codenexus", "index", "--path", "./repo", "--name", "repo"]);
        let sub5 = m5.subcommand_matches("index").unwrap();
        assert_eq!(
            sub5.get_one::<String>("ram_first").map(String::as_str),
            Some("false")
        );
        assert_eq!(
            sub5.get_one::<String>("force").map(String::as_str),
            Some("false")
        );

        let cmd6 = build_cli_command();
        let m6 = cmd6.get_matches_from(["codenexus", "detect_changes", "--path", "./repo"]);
        let sub6 = m6.subcommand_matches("detect_changes").unwrap();
        assert_eq!(
            sub6.get_one::<String>("mode").map(String::as_str),
            Some("unstaged")
        );
    }

    // --- extract_args: .codenexus/config.json command_defaults overlay ---

    fn trace_config_with_depth(value: &str) -> ProjectConfig {
        let mut defaults = HashMap::new();
        defaults.insert("depth".to_string(), value.to_string());
        let mut command_defaults = HashMap::new();
        command_defaults.insert("trace".to_string(), defaults);
        ProjectConfig {
            command_defaults,
            ..ProjectConfig::default()
        }
    }

    #[test]
    fn extract_args_config_default_overrides_sentinel_default() {
        let config = trace_config_with_depth("7");
        let cmd = build_cli_command();
        let m = cmd.get_matches_from([
            "codenexus",
            "trace",
            "--symbol",
            "main",
            "--trace_type",
            "calls",
        ]);
        let sub = m.subcommand_matches("trace").unwrap();
        let args = extract_args("trace", sub, &config);
        assert_eq!(args.get("depth").map(String::as_str), Some("7"));
        // Unconfigured sentinel args still carry their clap defaults.
        assert_eq!(args.get("path_filter").map(String::as_str), Some(""));
    }

    #[test]
    fn extract_args_explicit_cli_flag_beats_config_default() {
        let config = trace_config_with_depth("7");
        let cmd = build_cli_command();
        let m = cmd.get_matches_from([
            "codenexus",
            "trace",
            "--symbol",
            "main",
            "--trace_type",
            "calls",
            "--depth",
            "2",
        ]);
        let sub = m.subcommand_matches("trace").unwrap();
        let args = extract_args("trace", sub, &config);
        assert_eq!(args.get("depth").map(String::as_str), Some("2"));
    }

    // --- ProjectConfig parsing ---

    #[test]
    fn project_config_parses_full_schema() {
        let raw = r#"{
            "general": {"db": ".codenexus/team.lbug", "debounce_ms": 4000, "verbose": true},
            "command_defaults": {
                "index": {"ram_first": true, "force": false},
                "complexity": {"cyclomatic_red": 25}
            }
        }"#;
        let (cfg, warnings) = ProjectConfig::from_str(raw).expect("parse");
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        assert_eq!(cfg.db.as_deref(), Some(".codenexus/team.lbug"));
        assert_eq!(cfg.debounce_ms, Some(4000));
        assert_eq!(cfg.verbose, Some(true));
        assert_eq!(
            cfg.command_default("index", "ram_first").as_deref(),
            Some("true")
        );
        assert_eq!(
            cfg.command_default("index", "force").as_deref(),
            Some("false")
        );
        assert_eq!(
            cfg.command_default("complexity", "cyclomatic_red")
                .as_deref(),
            Some("25")
        );
        assert_eq!(cfg.command_default("trace", "depth"), None);
    }

    #[test]
    fn project_config_rejects_invalid_json() {
        assert!(ProjectConfig::from_str("{not json").is_err());
    }

    #[test]
    fn project_config_warns_on_unknown_keys_and_bad_types() {
        let raw = r#"{
            "general": {"db": 42, "typo": 1},
            "command_defaults": {"trace": {"depth": [1, 2]}},
            "extra_section": {}
        }"#;
        let (cfg, warnings) = ProjectConfig::from_str(raw).expect("parse");
        assert_eq!(cfg.db, None);
        assert_eq!(cfg.command_default("trace", "depth"), None);
        assert!(
            warnings.iter().any(|w| w.contains("general.db")),
            "expected db type warning, got: {warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("general.typo")),
            "expected unknown-key warning, got: {warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("command_defaults.trace.depth")),
            "expected array-value warning, got: {warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("extra_section")),
            "expected unknown-section warning, got: {warnings:?}"
        );
    }

    #[test]
    fn project_config_root_not_object_warns() {
        let (cfg, warnings) = ProjectConfig::from_str("[1, 2]").expect("parse");
        assert_eq!(cfg, ProjectConfig::default());
        assert!(warnings.iter().any(|w| w.contains("not a JSON object")));
    }

    #[test]
    #[serial_test::serial(db_discover)]
    fn project_config_load_missing_file_returns_default() {
        let tmp = tempfile::TempDir::new().expect("failed to create temp dir");
        let _guard = CwdGuard::enter(tmp.path());
        let (cfg, warnings) = ProjectConfig::load();
        assert_eq!(cfg, ProjectConfig::default());
        assert!(warnings.is_empty());
    }

    #[test]
    #[serial_test::serial(db_discover)]
    fn project_config_load_reads_codenexus_config_json() {
        let tmp = tempfile::TempDir::new().expect("failed to create temp dir");
        let dir = tmp.path().join(DEFAULT_DB_DIR);
        std::fs::create_dir_all(&dir).expect("failed to create .codenexus/");
        std::fs::write(
            dir.join("config.json"),
            r#"{"general": {"db": ".codenexus/x.lbug"}}"#,
        )
        .expect("write config.json");
        let _guard = CwdGuard::enter(tmp.path());
        let (cfg, warnings) = ProjectConfig::load();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(cfg.db.as_deref(), Some(".codenexus/x.lbug"));
    }
}
