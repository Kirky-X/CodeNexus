// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! CodeNexus binary entry point.
//!
//! Parses CLI arguments via [`clap`], builds a unified [`Kit`] from the
//! command's `--db` path, and dispatches to the matching handler in
//! [`codenexus::cli`]. Errors are printed to stderr and the process exits
//! with the PRD §4.1.6 exit code.

use std::path::PathBuf;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use codenexus::cli::{Cli, CliError, Command};
use codenexus::kit::{build_kit, KitBootstrapConfig};

// MCP server module (v0.3.0, T009) — sdforge-based MCP protocol exposure.
// Gated by the `mcp` feature. Replaces the hand-written JSON-RPC in
// `src/cli/mcp_cmd.rs` (which will be deleted in T016).
#[cfg(feature = "mcp")]
mod mcp;

/// Initialize the global `tracing` subscriber.
///
/// Events are filtered via the `RUST_LOG` environment variable ([`EnvFilter`])
/// and written to stdout with the `target` field omitted for concision. This
/// must be called once at startup so that `tracing::warn!`/`tracing::info!`
/// calls throughout the codebase are no longer silently dropped.
///
/// # Panics
/// Panics if a global subscriber has already been installed. `main` is the
/// only intended caller, so this is an acceptable failure mode.
pub fn init_logging() {
    tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .init();
}

/// Dispatches `command` to the matching `*_cmd::run` handler.
///
/// Builds a [`Kit`](codenexus::kit::Kit) from the command's `--db` path
/// (and `--debounce-ms` for the daemon command) before dispatching, so each
/// handler resolves its capabilities via `kit.require::<Key>()?` instead of
/// constructing subsystems ad-hoc.
///
/// # Errors
///
/// Returns [`CliError::Kit`] if the Kit cannot be built (e.g. invalid db
/// path). Each handler may return its own [`CliError`] variants.
fn run_command(command: Command) -> Result<(), CliError> {
    match command {
        Command::Index(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            let result = codenexus::cli::index_cmd::run(&kit, &args);
            // Leak the Kit intentionally. `build_kit` opens Storage / Query /
            // Trace Database connections at boot — BEFORE the indexer writes.
            // These connections hold stale MVCC snapshots of the empty DB.
            // If they drop at end-of-scope, LadybugDB's drop-time checkpoint
            // overwrites the indexer's writes with the stale empty view,
            // losing all data. `std::mem::forget` prevents the drop; the OS
            // reclaims file handles when the short-lived CLI process exits.
            //
            // Only the `index` command has this problem — it's the only
            // command that WRITES to the DB after the Kit opens connections.
            // Read-only commands (query/trace/impact/...) are unaffected
            // because there's nothing to corrupt.
            std::mem::forget(kit);
            result
        }
        Command::Query(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::query_cmd::run(&kit, &args)
        }
        Command::Trace(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::trace_cmd::run(&kit, &args)
        }
        Command::Impact(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::impact_cmd::run(&kit, &args)
        }
        Command::Search(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::search_cmd::run(&kit, &args)
        }
        #[cfg(feature = "daemon")]
        Command::Daemon(args) => {
            // Preserve CLI `--debounce-ms` by injecting it into the Kit config.
            let config = KitBootstrapConfig::new(PathBuf::from(&args.db))
                .with_debounce_ms(args.debounce_ms);
            let kit = build_kit(&config)?;
            codenexus::cli::daemon_cmd::run(&kit, &args)
        }
        Command::Status(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::status_cmd::run(&kit, &args)
        }
        Command::List(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::list_cmd::run(&kit, &args)
        }
        Command::Clean(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::clean_cmd::run(&kit, &args)
        }
        Command::Export(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::export_cmd::run(&kit, &args)
        }
        Command::Import(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::import_cmd::run(&kit, &args)
        }
        Command::Context(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::context_cmd::run(&kit, &args)
        }
        Command::DetectChanges(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::detect_changes_cmd::run(&kit, &args)
        }
        Command::Rename(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::rename_cmd::run(&kit, &args)
        }
        Command::Setup(args) => {
            // Setup writes MCP config files — no database access needed.
            codenexus::cli::setup_cmd::run(&args)
        }
        Command::Hook(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::hook_cmd::run(&kit, &args)
        }
        #[cfg(feature = "mcp")]
        Command::Mcp(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            mcp::run(kit, &args)
        }
        #[cfg(feature = "analysis")]
        Command::DeadCode(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::dead_code_cmd::run(&kit, &args)
        }
        #[cfg(feature = "analysis")]
        Command::Architecture(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::architecture_cmd::run(&kit, &args)
        }
        #[cfg(feature = "complexity")]
        Command::Complexity(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::complexity_cmd::run(&kit, &args)
        }
        #[cfg(feature = "api-review")]
        Command::ApiRouteMap(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::route_map_cmd::run(&kit, &args)
        }
        #[cfg(feature = "api-review")]
        Command::ApiShapeCheck(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::shape_check_cmd::run(&kit, &args)
        }
        #[cfg(feature = "api-review")]
        Command::ApiImpact(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::api_impact_cmd::run(&kit, &args)
        }
        #[cfg(feature = "api-review")]
        Command::ApiToolMap(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::tool_map_cmd::run(&kit, &args)
        }
        #[cfg(feature = "community")]
        Command::Community(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::community_cmd::run(&kit, &args)
        }
        #[cfg(feature = "cross-service")]
        Command::CrossService(args) => {
            let kit = build_kit(&KitBootstrapConfig::new(PathBuf::from(&args.db)))?;
            codenexus::cli::cross_service_cmd::run(&kit, &args)
        }
        #[cfg(feature = "lsp")]
        Command::LspGotoDef(args) => {
            // Ad-hoc LSP query — no database access needed (mirrors Setup).
            codenexus::cli::lsp_cmd::run_goto_def(&args)
        }
        #[cfg(feature = "lsp")]
        Command::LspHover(args) => {
            codenexus::cli::lsp_cmd::run_hover(&args)
        }
    }
}

fn main() {
    init_logging();
    let cli = Cli::parse();
    match run_command(cli.command) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(e.exit_code());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    /// A `MakeWriter` that buffers emitted events into a shared `Vec<u8>` so a
    /// test can assert on what the subscriber actually wrote.
    struct CapturingMakeWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl MakeWriter<'_> for CapturingMakeWriter {
        type Writer = CapturingWriter;

        fn make_writer(&self) -> Self::Writer {
            CapturingWriter {
                buf: self.buf.clone(),
            }
        }
    }

    struct CapturingWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for CapturingWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.buf.lock().unwrap().write_all(bytes)?;
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn fmt_subscriber_captures_tracing_events() {
        // Behavioral check that a subscriber built the same way `init_logging`
        // builds it (`FmtSubscriber` + `with_target(false)`) actually captures
        // emitted events. We use a scoped dispatcher (`with_default`) plus a
        // capturing writer instead of calling `init_logging` directly, because
        // the global default it installs is installable only once per process
        // and writes to stdout (which is not easily asserted on here). The
        // event-formatter under test is identical to the one `init_logging`
        // configures.
        let buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::FmtSubscriber::builder()
            .with_target(false)
            .with_writer(CapturingMakeWriter { buf: buf.clone() })
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("codenexus_test_marker");
        });

        let bytes = buf.lock().unwrap().clone();
        let captured = String::from_utf8(bytes).unwrap();
        assert!(
            captured.contains("codenexus_test_marker"),
            "expected captured output to contain the event message, got: {captured:?}"
        );
    }
}
