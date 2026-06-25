//! Command-line interface.
//!
//! Built on [`clap`] with subcommands for index/query/trace/impact/search/
//! daemon/status/list/clean. Each subcommand has its own `*_cmd` module with
//! a `run(args) -> Result<()>` entry point; [`main.rs`] dispatches to the
//! matching handler based on the parsed [`Command`].
//!
//! # Exit codes (PRD §4.1.6)
//!
//! | Code | Meaning              |
//! |------|----------------------|
//! | 0    | success              |
//! | 1    | input error          |
//! | 2    | database locked / IO |
//! | 3    | system error         |
//! | 4    | database corrupt     |
//!
//! See [`error::CliError`] for the full mapping.

pub mod args;
pub mod clean_cmd;
pub mod daemon_cmd;
pub mod error;
pub mod impact_cmd;
pub mod index_cmd;
pub mod list_cmd;
pub mod query_cmd;
pub mod search_cmd;
pub mod status_cmd;
pub mod trace_cmd;

pub use args::{Cli, Command};
pub use error::{CliError, Result};

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use crate::cli::args::{
        CleanArgs, DaemonArgs, ImpactArgs, IndexArgs, ListArgs, QueryArgs, SearchArgs, StatusArgs,
        TraceArgs,
    };
    use clap::Parser;

    /// Builds a `Cli` from a list of arguments and dispatches to the matching
    /// handler, returning the `Result<()>` just like `main.rs` does.
    ///
    /// This mirrors the dispatch logic in `main.rs` so we can test the full
    /// end-to-end flow without spawning a subprocess.
    fn dispatch(cli: Cli) -> Result<()> {
        match cli.command {
            Command::Index(args) => index_cmd::run(&args),
            Command::Query(args) => query_cmd::run(&args),
            Command::Trace(args) => trace_cmd::run(&args),
            Command::Impact(args) => impact_cmd::run(&args),
            Command::Search(args) => search_cmd::run(&args),
            Command::Daemon(args) => daemon_cmd::run(&args),
            Command::Status(args) => status_cmd::run(&args),
            Command::List(args) => list_cmd::run(&args),
            Command::Clean(args) => clean_cmd::run(&args),
        }
    }

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("cli_dispatch_testdb");
        std::mem::forget(dir);
        path
    }

    // --- Each subcommand dispatches to the right handler ---

    #[test]
    fn dispatch_index_calls_index_cmd() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn a() {}\n").unwrap();
        let db = fresh_db_path();
        let cli = Cli::parse_from([
            "codenexus",
            "index",
            tmp.path().to_str().unwrap(),
            "--name",
            "demo",
            "--db",
            db.to_str().unwrap(),
        ]);
        let result = dispatch(cli);
        assert!(
            result.is_ok(),
            "dispatch index should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn dispatch_query_calls_query_cmd() {
        let db = fresh_db_path();
        // Seed the database.
        {
            let conn = crate::storage::StorageConnection::open(&db).unwrap();
            conn.init_schema().unwrap();
            conn.execute("CREATE (:Project {id: 'p1', name: 'demo', rootPath: '/', language: 'rust', fileCount: 0, indexedAt: 0});").unwrap();
        }
        let cli = Cli::parse_from([
            "codenexus",
            "query",
            "MATCH (p:Project) RETURN p.name AS name;",
            "--db",
            db.to_str().unwrap(),
        ]);
        let result = dispatch(cli);
        assert!(
            result.is_ok(),
            "dispatch query should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn dispatch_daemon_calls_daemon_cmd() {
        // 使用不存在的路径，使 daemon_cmd::run 在路径校验阶段返回错误。
        // 这验证了 dispatch 正确调用了 daemon_cmd::run，且不会阻塞。
        let db = fresh_db_path();
        let cli = Cli::parse_from([
            "codenexus",
            "daemon",
            "/nonexistent/path/xyz",
            "--name",
            "demo",
            "--db",
            db.to_str().unwrap(),
        ]);
        let err = dispatch(cli).expect_err("nonexistent path should error");
        assert_eq!(err.exit_code(), 1, "输入错误 → 退出码 1");
    }

    #[test]
    fn dispatch_status_calls_status_cmd() {
        let db = fresh_db_path();
        // Initialize schema.
        {
            let conn = crate::storage::StorageConnection::open(&db).unwrap();
            conn.init_schema().unwrap();
        }
        let cli = Cli::parse_from(["codenexus", "status", "--db", db.to_str().unwrap()]);
        let result = dispatch(cli);
        assert!(
            result.is_ok(),
            "dispatch status should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn dispatch_list_calls_list_cmd() {
        let db = fresh_db_path();
        {
            let conn = crate::storage::StorageConnection::open(&db).unwrap();
            conn.init_schema().unwrap();
        }
        let cli = Cli::parse_from(["codenexus", "list", "--db", db.to_str().unwrap()]);
        let result = dispatch(cli);
        assert!(
            result.is_ok(),
            "dispatch list should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn dispatch_clean_calls_clean_cmd() {
        let db = fresh_db_path();
        {
            let repo = crate::storage::Repository::open(&db).unwrap();
            let node =
                crate::model::Node::builder(crate::model::NodeLabel::Project, "demo", "demo")
                    .id("p1")
                    .language(crate::model::Language::Rust)
                    .properties(serde_json::json!({
                        "rootPath": "/",
                        "fileCount": 0,
                        "indexedAt": 0,
                    }))
                    .build();
            repo.save_project(&node).unwrap();
        }
        let cli = Cli::parse_from(["codenexus", "clean", "demo", "--db", db.to_str().unwrap()]);
        let result = dispatch(cli);
        assert!(
            result.is_ok(),
            "dispatch clean should succeed: {:?}",
            result.err()
        );
    }

    // --- Exit codes propagate through dispatch ---

    #[test]
    fn dispatch_index_path_not_found_returns_exit_code_1() {
        let db = fresh_db_path();
        let cli = Cli::parse_from([
            "codenexus",
            "index",
            "/nonexistent/path/xyz",
            "--name",
            "demo",
            "--db",
            db.to_str().unwrap(),
        ]);
        let err = dispatch(cli).expect_err("path not found should error");
        assert_eq!(err.exit_code(), 1, "PRD §4.1.6: path not found → exit 1");
    }

    #[test]
    fn dispatch_clean_missing_project_returns_exit_code_1() {
        let db = fresh_db_path();
        {
            let conn = crate::storage::StorageConnection::open(&db).unwrap();
            conn.init_schema().unwrap();
        }
        let cli = Cli::parse_from([
            "codenexus",
            "clean",
            "nonexistent",
            "--db",
            db.to_str().unwrap(),
        ]);
        let err = dispatch(cli).expect_err("missing project should error");
        assert_eq!(err.exit_code(), 1, "ProjectNotFound → exit 1");
    }

    #[test]
    fn dispatch_trace_unknown_type_returns_exit_code_1() {
        let db = fresh_db_path();
        {
            let conn = crate::storage::StorageConnection::open(&db).unwrap();
            conn.init_schema().unwrap();
        }
        let cli = Cli::parse_from([
            "codenexus",
            "trace",
            "foo",
            "--type",
            "bogus",
            "--db",
            db.to_str().unwrap(),
        ]);
        let err = dispatch(cli).expect_err("unknown type should error");
        assert_eq!(err.exit_code(), 1, "InvalidInput → exit 1");
    }

    // --- End-to-end: index then query ---

    #[test]
    fn end_to_end_index_then_query() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn alpha() {}\nfn beta() {}\n").unwrap();
        let db = fresh_db_path();

        // Index.
        let cli = Cli::parse_from([
            "codenexus",
            "index",
            tmp.path().to_str().unwrap(),
            "--name",
            "demo",
            "--db",
            db.to_str().unwrap(),
        ]);
        dispatch(cli).expect("index should succeed");

        // Query the functions we just indexed.
        let cli = Cli::parse_from([
            "codenexus",
            "query",
            "MATCH (f:Function) RETURN f.name AS name ORDER BY f.name;",
            "--db",
            db.to_str().unwrap(),
        ]);
        dispatch(cli).expect("query should succeed");
    }

    // --- End-to-end: index then list then clean ---

    #[test]
    fn end_to_end_index_list_clean() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn alpha() {}\n").unwrap();
        let db = fresh_db_path();

        // Index.
        let cli = Cli::parse_from([
            "codenexus",
            "index",
            tmp.path().to_str().unwrap(),
            "--name",
            "demo",
            "--db",
            db.to_str().unwrap(),
        ]);
        dispatch(cli).expect("index should succeed");

        // List — should show one project.
        let cli = Cli::parse_from(["codenexus", "list", "--db", db.to_str().unwrap()]);
        dispatch(cli).expect("list should succeed");

        // Clean — should remove the project.
        let cli = Cli::parse_from(["codenexus", "clean", "demo", "--db", db.to_str().unwrap()]);
        dispatch(cli).expect("clean should succeed");

        // List again — should be empty.
        let cli = Cli::parse_from(["codenexus", "list", "--db", db.to_str().unwrap()]);
        dispatch(cli).expect("list after clean should succeed");
    }

    // --- Verify all arg structs are constructible (sanity) ---

    #[test]
    fn all_arg_structs_constructible() {
        let _ = IndexArgs {
            path: "/r".into(),
            name: "d".into(),
            db: "./x.lbug".into(),
            force: false,
            lsp: false,
            embed: false,
        };
        let _ = QueryArgs {
            cypher: "MATCH (n) RETURN n;".into(),
            db: "./x.lbug".into(),
            project: None,
        };
        let _ = TraceArgs {
            symbol: "s".into(),
            trace_type: "all".into(),
            depth: 3,
            db: "./x.lbug".into(),
        };
        let _ = ImpactArgs {
            symbol: "s".into(),
            depth: 3,
            db: "./x.lbug".into(),
        };
        let _ = SearchArgs {
            text: "t".into(),
            semantic: false,
            limit: 10,
            db: "./x.lbug".into(),
        };
        let _ = DaemonArgs {
            path: "/r".into(),
            name: "d".into(),
            debounce_ms: 2000,
            db: "./x.lbug".into(),
        };
        let _ = StatusArgs {
            db: "./x.lbug".into(),
        };
        let _ = ListArgs {
            db: "./x.lbug".into(),
        };
        let _ = CleanArgs {
            project: "p".into(),
            db: "./x.lbug".into(),
        };
    }
}
