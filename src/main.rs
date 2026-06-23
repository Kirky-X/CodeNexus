//! CodeNexus binary entry point.
//!
//! Parses CLI arguments via [`clap`] and dispatches to the matching handler
//! in [`codenexus::cli`]. Errors are printed to stderr and the process exits
//! with the PRD §4.1.6 exit code.

use clap::Parser;

use codenexus::cli::{Cli, Command};

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Index(args) => codenexus::cli::index_cmd::run(&args),
        Command::Query(args) => codenexus::cli::query_cmd::run(&args),
        Command::Trace(args) => codenexus::cli::trace_cmd::run(&args),
        Command::Impact(args) => codenexus::cli::impact_cmd::run(&args),
        Command::Search(args) => codenexus::cli::search_cmd::run(&args),
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
