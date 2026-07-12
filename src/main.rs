// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! CodeNexus binary entry point.
//!
//! CLI mode: sdforge `CliBuilder` dispatches to `#[service_api(cli = true)]`
//! handlers registered via `inventory` in the service layer.
//!
//! MCP mode (gated by `mcp` feature): sdforge MCP server serves
//! `#[service_api]` tools over stdio via rmcp.

use std::collections::HashMap;
use std::path::PathBuf;

use tracing_subscriber::EnvFilter;

use codenexus::service::error::CodeNexusError;
use codenexus::kit::{build_kit, KitBootstrapConfig};
use codenexus::service::init_kit;

/// Default database path when `--db` is not specified.
const DEFAULT_DB_PATH: &str = "./codenexus.lbug";

// `DEFAULT_DEBOUNCE_MS` is sourced from the daemon module when the `daemon`
// feature is enabled, or from the kit bootstrap fallback otherwise — avoiding
// a third hardcoded copy of the 2000ms default (BR-DAEMON-001).
#[cfg(feature = "daemon")]
use codenexus::daemon::DEFAULT_DEBOUNCE_MS;
#[cfg(not(feature = "daemon"))]
use codenexus::kit::bootstrap::DEFAULT_DEBOUNCE_MS;

/// Initialize the global `tracing` subscriber.
///
/// Events are filtered via `RUST_LOG` and written to stdout with the `target`
/// field omitted for concision.
pub fn init_logging() {
    tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .init();
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
        .build()
        .arg(
            clap::Arg::new("db")
                .long("db")
                .global(true)
                .default_value(DEFAULT_DB_PATH)
                .help("Database path"),
        )
        .arg(
            clap::Arg::new("debounce-ms")
                .long("debounce-ms")
                .global(true)
                .default_value(DEFAULT_DEBOUNCE_MS.to_string())
                .help("Daemon debounce interval (ms)"),
        );

    let matches = cmd.get_matches();

    let sub_name = match matches.subcommand_name() {
        Some(name) => name,
        None => {
            println!("Use --help to see available commands");
            return;
        }
    };
    let sub_matches = matches.subcommand_matches(sub_name).unwrap();

    let db = extract_global_arg(&matches, sub_matches, "db", DEFAULT_DB_PATH);
    let debounce_ms = extract_global_arg(
        &matches,
        sub_matches,
        "debounce-ms",
        &DEFAULT_DEBOUNCE_MS.to_string(),
    )
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
    top: &clap::ArgMatches,
    sub: &clap::ArgMatches,
    name: &str,
    fallback: &str,
) -> String {
    top.get_one::<String>(name)
        .or_else(|| sub.get_one::<String>(name))
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

/// Extracts subcommand args into a `HashMap<String, String>` for the handler.
fn extract_args(sub_name: &str, sub_matches: &clap::ArgMatches) -> HashMap<String, String> {
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
        .unwrap_or_else(|| DEFAULT_DB_PATH.to_string());

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
        use rmcp::ServiceExt;
        let server = sdforge::mcp::build();
        let transport = rmcp::transport::stdio();
        let service = server.serve(transport).await.expect("MCP serve error");
        service.waiting().await.expect("MCP service error");
    });
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

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
