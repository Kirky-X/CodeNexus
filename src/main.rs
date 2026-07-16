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

use codenexus::kit::{build_kit, KitBootstrapConfig};
use codenexus::service::error::CodeNexusError;
use codenexus::service::init_kit;

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

    let matches = cmd.get_matches();

    let sub_name = match matches.subcommand_name() {
        Some(name) => name,
        None => {
            eprintln!("Use --help to see available commands");
            std::process::exit(1);
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
    let config = KitBootstrapConfig::new(PathBuf::from(&db)).with_debounce_ms(debounce_ms);
    match runtime.block_on(build_kit(&config)) {
        Ok(kit) => {
            if let Err(e) = init_kit(kit) {
                eprintln!("[warn] Kit already initialized: {e}");
            }
        }
        Err(e) => {
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
    let raw_project = sub
        .get_one::<String>("name")
        .cloned()
        .or_else(|| {
            sub.get_one::<String>("path").map(|p| {
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
