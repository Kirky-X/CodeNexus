// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! CLI argument definitions (clap derive, PRD §4.1.3 / §4.2.3 / §4.4).
//!
//! Defines [`Cli`] (top-level parser) and [`Command`] (the 9 subcommands).
//! Each subcommand variant carries its own strongly-typed args struct so the
//! dispatch in [`crate::cli`] can hand them to the matching `*_cmd::run`
//! handler without re-parsing strings.

use clap::{Parser, Subcommand};

/// Top-level CLI parser.
///
/// Wraps [`Command`] so `main.rs` can call [`Cli::parse`] and dispatch on the
/// resulting subcommand.
#[derive(Parser, Debug)]
#[command(name = "codenexus", version, about = "Code knowledge graph indexing tool")]
pub struct Cli {
    /// The subcommand to execute.
    #[command(subcommand)]
    pub command: Command,
}

/// The 9 CLI subcommands (PRD §4.1.3, §4.2.3, §4.4).
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Index a codebase into the knowledge graph.
    Index(IndexArgs),
    /// Execute a Cypher query against the graph.
    Query(QueryArgs),
    /// Trace a symbol's call/data-flow paths.
    Trace(TraceArgs),
    /// Analyze the impact radius of changing a symbol.
    Impact(ImpactArgs),
    /// Search for symbols by name or content.
    Search(SearchArgs),
    /// Start the file-watching daemon (Task 15).
    #[cfg(feature = "daemon")]
    Daemon(DaemonArgs),
    /// Show indexing status for one or all projects.
    Status(StatusArgs),
    /// List all indexed projects.
    List(ListArgs),
    /// Remove a project and its index.
    Clean(CleanArgs),
    /// Export the graph database to a compressed team artifact (H7).
    Export(ExportArgs),
    /// Import a team artifact and optionally reindex local diff (H7).
    Import(ImportArgs),
    /// Show a 360° view of a symbol (H8).
    Context(ContextArgs),
    /// Detect symbols affected by uncommitted git changes (H8).
    DetectChanges(DetectChangesArgs),
    /// Propose graph + text edits for renaming a symbol (H8).
    Rename(RenameArgs),
}

/// Arguments for the `index` subcommand (PRD §4.1.3).
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct IndexArgs {
    /// Path to the codebase root to index.
    pub path: String,
    /// Project display name (also the multi-project isolation key).
    #[arg(long)]
    pub name: String,
    /// Database path (defaults to `./codenexus.lbug`).
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
    /// Force re-parse of every file, ignoring cached hashes.
    #[arg(long, default_value_t = false)]
    pub force: bool,
    /// Enable LSP-enhanced extraction (reserved for future use).
    #[arg(long, default_value_t = false, help = "Enable LSP-enhanced extraction (reserved for future use)（预留，当前未实现）")]
    pub lsp: bool,
    /// Enable embedding generation (requires the `embed` feature).
    #[arg(long, default_value_t = false)]
    pub embed: bool,
}

/// Arguments for the `query` subcommand (PRD §4.4).
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct QueryArgs {
    /// The Cypher query string to execute.
    pub cypher: String,
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
    /// Optional project name filter (informational; the query itself filters).
    #[arg(long)]
    pub project: Option<String>,
}

/// Arguments for the `trace` subcommand (PRD §4.2.3).
#[derive(Parser, Debug, Clone, PartialEq)]
pub struct TraceArgs {
    /// Symbol name or FQN to trace.
    pub symbol: String,
    /// Trace type: `calls`, `dataflow`, or `all` (default `all`).
    #[arg(long = "type", default_value = "all")]
    pub trace_type: String,
    /// Maximum trace depth (default 3).
    #[arg(long, default_value_t = 3)]
    pub depth: usize,
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
    /// Minimum edge confidence to include in trace (0.0–1.0).
    /// Edges with `confidence < min_confidence` are dropped before analysis.
    /// `--min-confidence 0.85` keeps only SameFile + ImportScoped edges
    /// (design.md D4).
    #[arg(long)]
    pub min_confidence: Option<f64>,
}

/// Arguments for the `impact` subcommand.
#[derive(Parser, Debug, Clone, PartialEq)]
pub struct ImpactArgs {
    /// Symbol name or FQN to analyze.
    pub symbol: String,
    /// Maximum reverse-traversal depth (default 3).
    #[arg(long, default_value_t = 3)]
    pub depth: usize,
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
    /// Minimum edge confidence to include in impact analysis (0.0–1.0).
    /// Edges with `confidence < min_confidence` are dropped before analysis.
    /// `--min-confidence 0.85` keeps only SameFile + ImportScoped edges
    /// (design.md D4).
    #[arg(long)]
    pub min_confidence: Option<f64>,
}

/// Arguments for the `search` subcommand (PRD §4.4).
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct SearchArgs {
    /// Search text (symbol name or content keyword).
    pub text: String,
    /// Use semantic (vector) search when available.
    #[arg(long, default_value_t = false)]
    pub semantic: bool,
    /// Maximum number of results to return (default 10).
    #[arg(long, default_value_t = 10)]
    pub limit: usize,
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
}

/// Arguments for the `daemon` subcommand (PRD §4.3, Task 15).
#[cfg(feature = "daemon")]
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct DaemonArgs {
    /// Path to the codebase root to watch.
    pub path: String,
    /// Project display name.
    #[arg(long)]
    pub name: String,
    /// Debounce window in milliseconds (default 2000, BR-DAEMON-001).
    #[arg(long, default_value_t = 2000)]
    pub debounce_ms: u64,
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
}

/// Arguments for the `status` subcommand.
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct StatusArgs {
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
}

/// Arguments for the `list` subcommand.
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct ListArgs {
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
}

/// Arguments for the `clean` subcommand.
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct CleanArgs {
    /// Project name (or id) to remove.
    pub project: String,
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
}

/// Arguments for the `export` subcommand (H7).
///
/// Dumps the LadybugDB database to a zstd-compressed team artifact
/// (`codenexus.graph.zst`). The artifact includes a JSON manifest with
/// codenexus version, export timestamp, and source DB path for integrity
/// verification on import.
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct ExportArgs {
    /// Output artifact path (defaults to `./codenexus.graph.zst`).
    #[arg(long, default_value = "./codenexus.graph.zst")]
    pub output: String,
    /// Database path to export.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
    /// Project name to include in the manifest (for multi-project isolation).
    #[arg(long)]
    pub project: Option<String>,
}

/// Arguments for the `import` subcommand (H7).
///
/// Decompresses a team artifact and loads it into a LadybugDB database.
/// Optionally triggers an incremental reindex of the local diff if `--reindex`
/// is given with a `--path` and `--name`.
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct ImportArgs {
    /// Input artifact path (defaults to `./codenexus.graph.zst`).
    #[arg(long, default_value = "./codenexus.graph.zst")]
    pub input: String,
    /// Database path to import into.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
    /// Trigger incremental reindex after import (requires --path and --name).
    #[arg(long, default_value_t = false)]
    pub reindex: bool,
    /// Codebase root path for reindex (used with --reindex).
    #[arg(long)]
    pub path: Option<String>,
    /// Project name for reindex (used with --reindex).
    #[arg(long)]
    pub name: Option<String>,
}

/// Arguments for the `context` subcommand (H8).
///
/// Shows a 360° view of a symbol: the resolved node, incoming edges
/// (callers/importers/readers/writers), outgoing edges (callees/imports/uses),
/// and processes/routes/endpoints the symbol participates in.
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct ContextArgs {
    /// Symbol name or FQN to inspect.
    pub symbol: String,
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
    /// BFS expansion depth for the surrounding subgraph (default 2).
    ///
    /// Controls how many hops of edges are loaded around the symbol. `1` shows
    /// only direct neighbors; `2` (default) shows neighbors-of-neighbors which
    /// is usually enough to spot the symbol's role in its module.
    #[arg(long, default_value_t = 2)]
    pub depth: usize,
}

/// Arguments for the `detect-changes` subcommand (H8).
///
/// Runs `git diff` in `--path` and maps each touched file/line range to the
/// symbols indexed in the graph, then classifies each affected symbol's
/// risk_level based on its incoming edge count.
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct DetectChangesArgs {
    /// Codebase root path (must be a git worktree).
    pub path: String,
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
    /// Git diff mode: `unstaged` (default), `staged`, or `head` (vs HEAD).
    #[arg(long, default_value = "unstaged")]
    pub mode: String,
}

/// Arguments for the `rename` subcommand (H8).
///
/// Proposes graph-edits for high-confidence edges and text-search edits for
/// review. Always runs in `--dry-run` mode by default; `--apply` writes the
/// text edits to disk (graph edits are applied via a subsequent `index` run).
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct RenameArgs {
    /// Current symbol name or FQN.
    pub from: String,
    /// New symbol name (must be a valid identifier in the symbol's language).
    pub to: String,
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
    /// Codebase root path (required for text-search edits).
    #[arg(long)]
    pub path: Option<String>,
    /// Apply text edits to disk (default: dry-run, only print the plan).
    #[arg(long, default_value_t = false)]
    pub apply: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    // --- Top-level Cli parsing ---

    #[test]
    fn cli_parses_index_subcommand() {
        let cli = Cli::parse_from([
            "codenexus",
            "index",
            "/repo",
            "--name",
            "demo",
        ]);
        match cli.command {
            Command::Index(args) => {
                assert_eq!(args.path, "/repo");
                assert_eq!(args.name, "demo");
                assert_eq!(args.db, "./codenexus.lbug");
                assert!(!args.force);
                assert!(!args.lsp);
                assert!(!args.embed);
            }
            other => panic!("expected Index, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_index_with_all_flags() {
        let cli = Cli::parse_from([
            "codenexus",
            "index",
            "/repo",
            "--name",
            "demo",
            "--db",
            "/tmp/db.lbug",
            "--force",
            "--lsp",
            "--embed",
        ]);
        match cli.command {
            Command::Index(args) => {
                assert_eq!(args.path, "/repo");
                assert_eq!(args.name, "demo");
                assert_eq!(args.db, "/tmp/db.lbug");
                assert!(args.force);
                assert!(args.lsp);
                assert!(args.embed);
            }
            other => panic!("expected Index, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_query_subcommand() {
        let cli = Cli::parse_from([
            "codenexus",
            "query",
            "MATCH (f:Function) RETURN f.name;",
        ]);
        match cli.command {
            Command::Query(args) => {
                assert_eq!(args.cypher, "MATCH (f:Function) RETURN f.name;");
                assert_eq!(args.db, "./codenexus.lbug");
                assert!(args.project.is_none());
            }
            other => panic!("expected Query, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_query_with_project() {
        let cli = Cli::parse_from([
            "codenexus",
            "query",
            "MATCH (f:Function) RETURN f.name;",
            "--project",
            "demo",
        ]);
        match cli.command {
            Command::Query(args) => {
                assert_eq!(args.project.as_deref(), Some("demo"));
            }
            other => panic!("expected Query, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_trace_subcommand_defaults() {
        let cli = Cli::parse_from(["codenexus", "trace", "main"]);
        match cli.command {
            Command::Trace(args) => {
                assert_eq!(args.symbol, "main");
                assert_eq!(args.trace_type, "all");
                assert_eq!(args.depth, 3);
                assert_eq!(args.db, "./codenexus.lbug");
            }
            other => panic!("expected Trace, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_trace_with_type_and_depth() {
        let cli = Cli::parse_from([
            "codenexus", "trace", "main", "--type", "calls", "--depth", "5",
        ]);
        match cli.command {
            Command::Trace(args) => {
                assert_eq!(args.trace_type, "calls");
                assert_eq!(args.depth, 5);
            }
            other => panic!("expected Trace, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_impact_subcommand() {
        let cli = Cli::parse_from(["codenexus", "impact", "helper"]);
        match cli.command {
            Command::Impact(args) => {
                assert_eq!(args.symbol, "helper");
                assert_eq!(args.depth, 3);
                assert_eq!(args.db, "./codenexus.lbug");
            }
            other => panic!("expected Impact, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_impact_with_depth() {
        let cli = Cli::parse_from(["codenexus", "impact", "helper", "--depth", "10"]);
        match cli.command {
            Command::Impact(args) => {
                assert_eq!(args.depth, 10);
            }
            other => panic!("expected Impact, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_trace_with_min_confidence() {
        let cli = Cli::parse_from([
            "codenexus", "trace", "main", "--min-confidence", "0.85",
        ]);
        match cli.command {
            Command::Trace(args) => {
                assert!((args.min_confidence.unwrap() - 0.85).abs() < f64::EPSILON);
            }
            other => panic!("expected Trace, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_impact_with_min_confidence() {
        let cli = Cli::parse_from([
            "codenexus", "impact", "helper", "--min-confidence", "0.90",
        ]);
        match cli.command {
            Command::Impact(args) => {
                assert!((args.min_confidence.unwrap() - 0.90).abs() < f64::EPSILON);
            }
            other => panic!("expected Impact, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_trace_min_confidence_defaults_to_none() {
        let cli = Cli::parse_from(["codenexus", "trace", "main"]);
        match cli.command {
            Command::Trace(args) => {
                assert!(args.min_confidence.is_none());
            }
            other => panic!("expected Trace, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_search_subcommand_defaults() {
        let cli = Cli::parse_from(["codenexus", "search", "parse"]);
        match cli.command {
            Command::Search(args) => {
                assert_eq!(args.text, "parse");
                assert!(!args.semantic);
                assert_eq!(args.limit, 10);
                assert_eq!(args.db, "./codenexus.lbug");
            }
            other => panic!("expected Search, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_search_with_semantic_and_limit() {
        let cli = Cli::parse_from([
            "codenexus", "search", "parse", "--semantic", "--limit", "50",
        ]);
        match cli.command {
            Command::Search(args) => {
                assert!(args.semantic);
                assert_eq!(args.limit, 50);
            }
            other => panic!("expected Search, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "daemon")]
    fn cli_parses_daemon_subcommand() {
        let cli = Cli::parse_from([
            "codenexus", "daemon", "/repo", "--name", "demo",
        ]);
        match cli.command {
            Command::Daemon(args) => {
                assert_eq!(args.path, "/repo");
                assert_eq!(args.name, "demo");
                assert_eq!(args.debounce_ms, 2000);
                assert_eq!(args.db, "./codenexus.lbug");
            }
            other => panic!("expected Daemon, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "daemon")]
    fn cli_parses_daemon_with_debounce() {
        let cli = Cli::parse_from([
            "codenexus", "daemon", "/repo", "--name", "demo", "--debounce-ms", "500",
        ]);
        match cli.command {
            Command::Daemon(args) => {
                assert_eq!(args.debounce_ms, 500);
            }
            other => panic!("expected Daemon, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_status_subcommand() {
        let cli = Cli::parse_from(["codenexus", "status"]);
        match cli.command {
            Command::Status(args) => {
                assert_eq!(args.db, "./codenexus.lbug");
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_status_with_db() {
        let cli = Cli::parse_from(["codenexus", "status", "--db", "/tmp/x.lbug"]);
        match cli.command {
            Command::Status(args) => {
                assert_eq!(args.db, "/tmp/x.lbug");
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_list_subcommand() {
        let cli = Cli::parse_from(["codenexus", "list"]);
        match cli.command {
            Command::List(args) => {
                assert_eq!(args.db, "./codenexus.lbug");
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_clean_subcommand() {
        let cli = Cli::parse_from(["codenexus", "clean", "demo"]);
        match cli.command {
            Command::Clean(args) => {
                assert_eq!(args.project, "demo");
                assert_eq!(args.db, "./codenexus.lbug");
            }
            other => panic!("expected Clean, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_clean_with_db() {
        let cli = Cli::parse_from([
            "codenexus", "clean", "demo", "--db", "/tmp/x.lbug",
        ]);
        match cli.command {
            Command::Clean(args) => {
                assert_eq!(args.project, "demo");
                assert_eq!(args.db, "/tmp/x.lbug");
            }
            other => panic!("expected Clean, got {other:?}"),
        }
    }

    // --- Required-arg validation ---

    #[test]
    fn index_requires_name_flag() {
        let result = Cli::try_parse_from(["codenexus", "index", "/repo"]);
        assert!(result.is_err(), "index without --name should fail");
    }

    #[test]
    fn index_requires_path_arg() {
        let result = Cli::try_parse_from([
            "codenexus", "index", "--name", "demo",
        ]);
        assert!(result.is_err(), "index without path should fail");
    }

    #[test]
    fn query_requires_cypher_arg() {
        let result = Cli::try_parse_from(["codenexus", "query"]);
        assert!(result.is_err(), "query without cypher should fail");
    }

    #[test]
    fn trace_requires_symbol_arg() {
        let result = Cli::try_parse_from(["codenexus", "trace"]);
        assert!(result.is_err(), "trace without symbol should fail");
    }

    #[test]
    fn unknown_subcommand_fails() {
        let result = Cli::try_parse_from(["codenexus", "bogus"]);
        assert!(result.is_err(), "unknown subcommand should fail");
    }

    #[test]
    fn no_subcommand_fails() {
        let result = Cli::try_parse_from(["codenexus"]);
        assert!(result.is_err(), "no subcommand should fail");
    }

    // --- Debug / Clone / PartialEq on arg structs ---

    #[test]
    fn index_args_clone_eq() {
        let a = IndexArgs {
            path: "/r".into(),
            name: "d".into(),
            db: "./x.lbug".into(),
            force: true,
            lsp: false,
            embed: false,
        };
        assert_eq!(a, a.clone());
    }

    #[test]
    fn query_args_clone_eq() {
        let a = QueryArgs {
            cypher: "MATCH (n) RETURN n;".into(),
            db: "./x.lbug".into(),
            project: Some("demo".into()),
        };
        assert_eq!(a, a.clone());
    }

    #[test]
    fn trace_args_clone_eq() {
        let a = TraceArgs {
            symbol: "main".into(),
            trace_type: "calls".into(),
            depth: 5,
            db: "./x.lbug".into(),
            min_confidence: None,
        };
        assert_eq!(a, a.clone());
    }

    #[test]
    fn impact_args_clone_eq() {
        let a = ImpactArgs {
            symbol: "x".into(),
            depth: 2,
            db: "./x.lbug".into(),
            min_confidence: None,
        };
        assert_eq!(a, a.clone());
    }

    #[test]
    fn search_args_clone_eq() {
        let a = SearchArgs {
            text: "q".into(),
            semantic: true,
            limit: 20,
            db: "./x.lbug".into(),
        };
        assert_eq!(a, a.clone());
    }

    #[test]
    #[cfg(feature = "daemon")]
    fn daemon_args_clone_eq() {
        let a = DaemonArgs {
            path: "/r".into(),
            name: "d".into(),
            debounce_ms: 100,
            db: "./x.lbug".into(),
        };
        assert_eq!(a, a.clone());
    }

    #[test]
    fn status_args_clone_eq() {
        let a = StatusArgs {
            db: "./x.lbug".into(),
        };
        assert_eq!(a, a.clone());
    }

    #[test]
    fn list_args_clone_eq() {
        let a = ListArgs {
            db: "./x.lbug".into(),
        };
        assert_eq!(a, a.clone());
    }

    #[test]
    fn clean_args_clone_eq() {
        let a = CleanArgs {
            project: "demo".into(),
            db: "./x.lbug".into(),
        };
        assert_eq!(a, a.clone());
    }

    #[test]
    fn args_debug_contains_struct_name() {
        let a = IndexArgs {
            path: "/r".into(),
            name: "d".into(),
            db: "./x.lbug".into(),
            force: false,
            lsp: false,
            embed: false,
        };
        let s = format!("{a:?}");
        assert!(s.contains("IndexArgs"));
    }

    // --- Export / Import (H7) ---

    #[test]
    fn cli_parses_export_subcommand_defaults() {
        let cli = Cli::parse_from(["codenexus", "export"]);
        match cli.command {
            Command::Export(args) => {
                assert_eq!(args.output, "./codenexus.graph.zst");
                assert_eq!(args.db, "./codenexus.lbug");
                assert!(args.project.is_none());
            }
            other => panic!("expected Export, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_export_with_output_db_project() {
        let cli = Cli::parse_from([
            "codenexus",
            "export",
            "--output",
            "/tmp/graph.zst",
            "--db",
            "/tmp/x.lbug",
            "--project",
            "demo",
        ]);
        match cli.command {
            Command::Export(args) => {
                assert_eq!(args.output, "/tmp/graph.zst");
                assert_eq!(args.db, "/tmp/x.lbug");
                assert_eq!(args.project.as_deref(), Some("demo"));
            }
            other => panic!("expected Export, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_import_subcommand_defaults() {
        let cli = Cli::parse_from(["codenexus", "import"]);
        match cli.command {
            Command::Import(args) => {
                assert_eq!(args.input, "./codenexus.graph.zst");
                assert_eq!(args.db, "./codenexus.lbug");
                assert!(!args.reindex);
                assert!(args.path.is_none());
                assert!(args.name.is_none());
            }
            other => panic!("expected Import, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_import_with_reindex_and_path() {
        let cli = Cli::parse_from([
            "codenexus",
            "import",
            "--input",
            "/tmp/graph.zst",
            "--db",
            "/tmp/x.lbug",
            "--reindex",
            "--path",
            "/repo",
            "--name",
            "demo",
        ]);
        match cli.command {
            Command::Import(args) => {
                assert_eq!(args.input, "/tmp/graph.zst");
                assert_eq!(args.db, "/tmp/x.lbug");
                assert!(args.reindex);
                assert_eq!(args.path.as_deref(), Some("/repo"));
                assert_eq!(args.name.as_deref(), Some("demo"));
            }
            other => panic!("expected Import, got {other:?}"),
        }
    }

    #[test]
    fn export_args_clone_eq() {
        let a = ExportArgs {
            output: "/tmp/o.zst".into(),
            db: "/tmp/x.lbug".into(),
            project: Some("demo".into()),
        };
        assert_eq!(a, a.clone());
    }

    #[test]
    fn import_args_clone_eq() {
        let a = ImportArgs {
            input: "/tmp/i.zst".into(),
            db: "/tmp/x.lbug".into(),
            reindex: true,
            path: Some("/r".into()),
            name: Some("d".into()),
        };
        assert_eq!(a, a.clone());
    }

    // --- Context / DetectChanges / Rename (H8) ---

    #[test]
    fn cli_parses_context_subcommand_defaults() {
        let cli = Cli::parse_from(["codenexus", "context", "helper"]);
        match cli.command {
            Command::Context(args) => {
                assert_eq!(args.symbol, "helper");
                assert_eq!(args.depth, 2);
                assert_eq!(args.db, "./codenexus.lbug");
            }
            other => panic!("expected Context, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_context_with_depth_and_db() {
        let cli = Cli::parse_from([
            "codenexus",
            "context",
            "helper",
            "--depth",
            "5",
            "--db",
            "/tmp/x.lbug",
        ]);
        match cli.command {
            Command::Context(args) => {
                assert_eq!(args.symbol, "helper");
                assert_eq!(args.depth, 5);
                assert_eq!(args.db, "/tmp/x.lbug");
            }
            other => panic!("expected Context, got {other:?}"),
        }
    }

    #[test]
    fn context_requires_symbol_arg() {
        let result = Cli::try_parse_from(["codenexus", "context"]);
        assert!(result.is_err(), "context without symbol should fail");
    }

    #[test]
    fn cli_parses_detect_changes_subcommand_defaults() {
        let cli = Cli::parse_from(["codenexus", "detect-changes", "/repo"]);
        match cli.command {
            Command::DetectChanges(args) => {
                assert_eq!(args.path, "/repo");
                assert_eq!(args.mode, "unstaged");
                assert_eq!(args.db, "./codenexus.lbug");
            }
            other => panic!("expected DetectChanges, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_detect_changes_with_mode_and_db() {
        let cli = Cli::parse_from([
            "codenexus",
            "detect-changes",
            "/repo",
            "--mode",
            "staged",
            "--db",
            "/tmp/x.lbug",
        ]);
        match cli.command {
            Command::DetectChanges(args) => {
                assert_eq!(args.path, "/repo");
                assert_eq!(args.mode, "staged");
                assert_eq!(args.db, "/tmp/x.lbug");
            }
            other => panic!("expected DetectChanges, got {other:?}"),
        }
    }

    #[test]
    fn detect_changes_requires_path_arg() {
        let result = Cli::try_parse_from(["codenexus", "detect-changes"]);
        assert!(result.is_err(), "detect-changes without path should fail");
    }

    #[test]
    fn cli_parses_rename_subcommand_defaults() {
        let cli = Cli::parse_from(["codenexus", "rename", "old", "new"]);
        match cli.command {
            Command::Rename(args) => {
                assert_eq!(args.from, "old");
                assert_eq!(args.to, "new");
                assert_eq!(args.db, "./codenexus.lbug");
                assert!(args.path.is_none());
                assert!(!args.apply);
            }
            other => panic!("expected Rename, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_rename_with_path_and_apply() {
        let cli = Cli::parse_from([
            "codenexus",
            "rename",
            "old",
            "new",
            "--db",
            "/tmp/x.lbug",
            "--path",
            "/repo",
            "--apply",
        ]);
        match cli.command {
            Command::Rename(args) => {
                assert_eq!(args.from, "old");
                assert_eq!(args.to, "new");
                assert_eq!(args.db, "/tmp/x.lbug");
                assert_eq!(args.path.as_deref(), Some("/repo"));
                assert!(args.apply);
            }
            other => panic!("expected Rename, got {other:?}"),
        }
    }

    #[test]
    fn rename_requires_two_args() {
        let result = Cli::try_parse_from(["codenexus", "rename", "only_one"]);
        assert!(result.is_err(), "rename with only one arg should fail");
    }

    #[test]
    fn context_args_clone_eq() {
        let a = ContextArgs {
            symbol: "s".into(),
            db: "/tmp/x.lbug".into(),
            depth: 4,
        };
        assert_eq!(a, a.clone());
    }

    #[test]
    fn detect_changes_args_clone_eq() {
        let a = DetectChangesArgs {
            path: "/r".into(),
            db: "/tmp/x.lbug".into(),
            mode: "head".into(),
        };
        assert_eq!(a, a.clone());
    }

    #[test]
    fn rename_args_clone_eq() {
        let a = RenameArgs {
            from: "a".into(),
            to: "b".into(),
            db: "/tmp/x.lbug".into(),
            path: Some("/r".into()),
            apply: true,
        };
        assert_eq!(a, a.clone());
    }
}
