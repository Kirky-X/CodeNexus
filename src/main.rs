// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! CodeNexus binary entry point.
//!
//! Parses CLI arguments via [`clap`] and dispatches to the matching handler
//! in [`codenexus::cli`]. Errors are printed to stderr and the process exits
//! with the PRD §4.1.6 exit code.

use clap::Parser;
use tracing_subscriber::EnvFilter;

use codenexus::cli::{Cli, Command};

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

fn main() {
    init_logging();
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Index(args) => codenexus::cli::index_cmd::run(&args),
        Command::Query(args) => codenexus::cli::query_cmd::run(&args),
        Command::Trace(args) => codenexus::cli::trace_cmd::run(&args),
        Command::Impact(args) => codenexus::cli::impact_cmd::run(&args),
        Command::Search(args) => codenexus::cli::search_cmd::run(&args),
        #[cfg(feature = "daemon")]
        Command::Daemon(args) => codenexus::cli::daemon_cmd::run(&args),
        Command::Status(args) => codenexus::cli::status_cmd::run(&args),
        Command::List(args) => codenexus::cli::list_cmd::run(&args),
        Command::Clean(args) => codenexus::cli::clean_cmd::run(&args),
    };
    match result {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(e.exit_code());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    #[test]
    fn init_logging_is_callable() {
        // `init_logging` installs a *process-global* subscriber via
        // `set_global_default`, which can only succeed once and writes to
        // stdout. Calling it here would both pollute other tests and panic on
        // any second invocation. We therefore verify the public contract
        // instead: the symbol exists, is reachable, and has the `fn() -> ()`
        // signature. If it is removed, renamed, or its signature changes, this
        // assignment fails to compile.
        let _: fn() = init_logging;
    }

    /// A `MakeWriter` that buffers emitted events into a shared `Vec<u8>` so a
    /// test can assert on what the subscriber actually wrote.
    struct CapturingMakeWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl MakeWriter for CapturingMakeWriter {
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
