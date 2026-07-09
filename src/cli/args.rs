// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! CLI argument definitions (clap derive, PRD §4.1.3 / §4.2.3 / §4.4).
//!
//! Defines [`Cli`] (top-level parser) and [`Command`] (the 17 CLI subcommands
//! (9 PRD + 8 agent integration)). Each subcommand variant carries its own
//! strongly-typed args struct so the dispatch in [`crate::cli`] can hand them
//! to the matching `*_cmd::run` handler without re-parsing strings.

use clap::{Parser, Subcommand};

#[cfg(feature = "complexity")]
use crate::analysis::complexity::{SpaceComplexity, TimeComplexity};

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

/// The 17 CLI subcommands (9 PRD + 8 agent integration) (PRD §4.1.3, §4.2.3, §4.4, §4.5).
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
    /// Auto-detect installed AI agents and write MCP config (H13).
    Setup(SetupArgs),
    /// Emit PreToolUse/PostToolUse hook JSON (H13).
    Hook(HookArgs),
    /// Serve MCP tools over stdio (H13).
    Mcp(McpArgs),
    /// Detect dead code (zero-indegree CALLS functions) for a project (T005).
    #[cfg(feature = "analysis")]
    DeadCode(DeadCodeArgs),
    /// Show a project's architecture overview (T006, v0.1.6).
    #[cfg(feature = "analysis")]
    Architecture(ArchitectureArgs),
    /// Analyse code complexity metrics (cyclomatic, cognitive, nesting, length) (T-v0.2.1).
    #[cfg(feature = "complexity")]
    Complexity(ComplexityArgs),
    /// List all API routes + handlers + middleware (T008, v0.2.0).
    #[cfg(feature = "api-review")]
    ApiRouteMap(RouteMapArgs),
    /// Check API endpoint schema consistency (T008, v0.2.0).
    #[cfg(feature = "api-review")]
    ApiShapeCheck(ShapeCheckArgs),
    /// Analyse the impact of changing an endpoint (T008, v0.2.0).
    #[cfg(feature = "api-review")]
    ApiImpact(ApiImpactArgs),
    /// List all MCP tools + their handlers (T008, v0.2.0).
    #[cfg(feature = "api-review")]
    ApiToolMap(ToolMapArgs),
    /// Detect communities in the CALLS graph via Louvain (T009, v0.2.0).
    #[cfg(feature = "community")]
    Community(CommunityArgs),
    /// Detect cross-service links via route pattern matching (T010, v0.2.0).
    #[cfg(feature = "cross-service")]
    CrossService(CrossServiceArgs),
    /// Query LSP Go-to-Definition for a Rust symbol (T007, v0.2.0).
    #[cfg(feature = "lsp")]
    LspGotoDef(LspGotoDefArgs),
    /// Query LSP Hover info for a Rust symbol (T007, v0.2.0).
    #[cfg(feature = "lsp")]
    LspHover(LspHoverArgs),
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
    /// Enable LSP-enhanced semantic type extraction (requires `rust-analyzer`
    /// on PATH for Rust; degrades gracefully to pure tree-sitter if the
    /// server is missing or a query times out — never aborts the index).
    #[arg(long, default_value_t = false)]
    pub lsp: bool,
    /// Enable embedding generation (requires the `embed` feature).
    #[arg(long, default_value_t = false)]
    pub embed: bool,
    /// RAM-first indexing (H15): LZ4-compress source files into memory, parse
    /// from memory, then single `COPY FROM` dump. Recommended for
    /// small-to-medium repositories (< 1 GB source). Default is streaming.
    #[arg(long, default_value_t = false)]
    pub ram_first: bool,
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
    /// Narrow disambiguation by node UID (H14).
    #[arg(long)]
    pub uid: Option<String>,
    /// Narrow disambiguation by file path (H14).
    #[arg(long)]
    pub file: Option<String>,
    /// Narrow disambiguation by node label, e.g. `"Function"` (H14).
    #[arg(long)]
    pub kind: Option<String>,
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
    /// Narrow disambiguation by node UID (H14).
    #[arg(long)]
    pub uid: Option<String>,
    /// Narrow disambiguation by file path (H14).
    #[arg(long)]
    pub file: Option<String>,
    /// Narrow disambiguation by node label, e.g. `"Function"` (H14).
    #[arg(long)]
    pub kind: Option<String>,
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
    /// Narrow search by node UID — looks up the node directly (H14).
    #[arg(long)]
    pub uid: Option<String>,
    /// Narrow search by file path (H14).
    #[arg(long)]
    pub file: Option<String>,
    /// Narrow search by node label, e.g. `"Function"` (H14).
    #[arg(long)]
    pub kind: Option<String>,
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

/// Arguments for the `setup` subcommand (H13).
///
/// Auto-detects installed AI coding agents (Claude Code, Cursor, Codex) under
/// `$HOME` and writes the MCP server config for `codenexus mcp` into each
/// agent's config file. Existing entries pointing to a different binary prompt
/// for confirmation unless `--force` is given.
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct SetupArgs {
    /// Skip confirmation prompts and overwrite existing entries without asking.
    #[arg(long, default_value_t = false)]
    pub force: bool,
}

/// Arguments for the `hook` subcommand (H13).
///
/// Reads a PreToolUse/PostToolUse JSON payload from stdin and emits a
/// no-op acknowledgment. The hook always exits 0, never blocks a tool call,
/// and never intercepts `Read` tool invocations.
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct HookArgs {
    /// Database path (used for PostToolUse summarisation of `codenexus rename`).
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
}

/// Arguments for the `mcp` subcommand (H13).
///
/// Starts the MCP stdio server, exposing query/trace/impact/search/context as
/// MCP tools. Launched by AI agents via the config written by `codenexus setup`.
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct McpArgs {
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
}

/// Arguments for the `dead-code` subcommand (T005, v0.1.5).
///
/// Detects `Function`/`Method` nodes with zero incoming `CALLS` edges that
/// are not entry points or test functions. Output is a JSON array of
/// [`DeadCodeEntry`](crate::analysis::dead_code::DeadCodeEntry) objects.
#[cfg(feature = "analysis")]
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct DeadCodeArgs {
    /// Project name (the multi-project isolation key).
    pub project: String,
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
    /// Additional entry-point glob patterns (e.g. `main`, `__main__`).
    /// Functions matching these patterns are excluded from dead-code results.
    /// Test functions (`test_*`, `*_test`, `*_spec`) are always excluded.
    #[arg(long)]
    pub entry: Option<Vec<String>>,
}

/// Arguments for the `architecture` subcommand (T006, v0.1.6).
///
/// Produces a high-level overview of a project's structure: language
/// distribution, package structure, entry points, HTTP routes, and hotspot
/// functions. Output is a JSON object with `languages`, `packages`,
/// `entry_points`, `routes`, and `hotspots` arrays.
#[cfg(feature = "analysis")]
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct ArchitectureArgs {
    /// Project name (the multi-project isolation key).
    pub project: String,
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
}

/// Arguments for the `complexity` subcommand (v0.3.1).
///
/// Calculates AST-based complexity metrics (cyclomatic, cognitive, nesting
/// depth, function length) for all functions in a project. Output is a JSON
/// object with `complexity` array and `summary` statistics.
#[cfg(feature = "complexity")]
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct ComplexityArgs {
    /// Project name (the multi-project isolation key).
    pub project: String,
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
    /// Only show Red and Critical high-risk functions.
    #[arg(long)]
    pub red_only: bool,
    /// Sort output by overall severity (Critical first).
    #[arg(long)]
    pub sort_by_severity: bool,
    /// Cyclomatic complexity green threshold (overrides default when set).
    #[arg(long)]
    pub cyclomatic_green: Option<u32>,
    /// Cyclomatic complexity yellow threshold (overrides default when set).
    #[arg(long)]
    pub cyclomatic_yellow: Option<u32>,
    /// Cyclomatic complexity red threshold (overrides default when set).
    #[arg(long)]
    pub cyclomatic_red: Option<u32>,
    /// Cognitive complexity green threshold (overrides default when set).
    #[arg(long)]
    pub cognitive_green: Option<u32>,
    /// Cognitive complexity yellow threshold (overrides default when set).
    #[arg(long)]
    pub cognitive_yellow: Option<u32>,
    /// Cognitive complexity red threshold (overrides default when set).
    #[arg(long)]
    pub cognitive_red: Option<u32>,
    /// Nesting depth green threshold (overrides default when set).
    #[arg(long)]
    pub nesting_green: Option<u32>,
    /// Nesting depth yellow threshold (overrides default when set).
    #[arg(long)]
    pub nesting_yellow: Option<u32>,
    /// Nesting depth red threshold (overrides default when set).
    #[arg(long)]
    pub nesting_red: Option<u32>,
    /// Function length green threshold (overrides default when set).
    #[arg(long)]
    pub func_length_green: Option<u32>,
    /// Function length yellow threshold (overrides default when set).
    #[arg(long)]
    pub func_length_yellow: Option<u32>,
    /// Function length red threshold (overrides default when set).
    #[arg(long)]
    pub func_length_red: Option<u32>,
    /// Halstead volume green threshold (overrides default when set).
    #[arg(long)]
    pub halstead_volume_green: Option<u32>,
    /// Halstead volume yellow threshold (overrides default when set).
    #[arg(long)]
    pub halstead_volume_yellow: Option<u32>,
    /// Halstead volume red threshold (overrides default when set).
    #[arg(long)]
    pub halstead_volume_red: Option<u32>,
    /// Maintainability Index green minimum (overrides default when set).
    #[arg(long)]
    pub maintainability_green: Option<u32>,
    /// Maintainability Index yellow minimum (overrides default when set).
    #[arg(long)]
    pub maintainability_yellow: Option<u32>,
    /// Maintainability Index red minimum (overrides default when set).
    #[arg(long)]
    pub maintainability_red: Option<u32>,
    /// Time complexity green class (e.g. `O(log n)`); overrides default when set.
    #[arg(long)]
    pub time_complexity_green: Option<TimeComplexity>,
    /// Time complexity yellow class (e.g. `O(n)`); overrides default when set.
    #[arg(long)]
    pub time_complexity_yellow: Option<TimeComplexity>,
    /// Time complexity red class (e.g. `O(n^2)`); overrides default when set.
    #[arg(long)]
    pub time_complexity_red: Option<TimeComplexity>,
    /// Space complexity yellow class (e.g. `O(1)`); overrides default when set.
    #[arg(long)]
    pub space_complexity_yellow: Option<SpaceComplexity>,
    /// Space complexity red class (e.g. `O(n)`); overrides default when set.
    #[arg(long)]
    pub space_complexity_red: Option<SpaceComplexity>,
}

/// Arguments for the `api-route-map` subcommand (T008, v0.2.0).
///
/// Lists all `Route`/`Endpoint` nodes for a project joined with their handler
/// functions and middleware chains via `HANDLES`/`USES` edges. Output is a
/// JSON object `{ project, route_map: [...] }`.
#[cfg(feature = "api-review")]
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct RouteMapArgs {
    /// Project name (the multi-project isolation key).
    pub project: String,
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
}

/// Arguments for the `api-shape-check` subcommand (T008, v0.2.0).
///
/// Validates API endpoint schema consistency by comparing each `Endpoint`
/// node's `expectedSchema` property with the actual schema recorded on `CALLS`
/// edges pointing to it. Output is a JSON object `{ project, violations: [...] }`.
#[cfg(feature = "api-review")]
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct ShapeCheckArgs {
    /// Project name (the multi-project isolation key).
    pub project: String,
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
}

/// Arguments for the `api-impact` subcommand (T008, v0.2.0).
///
/// Traces which callers would be affected by changing an endpoint, by
/// reverse-traversing `CALLS` edges from the endpoint's handler. `endpoint`
/// is matched against both `Endpoint.path` and `Endpoint.name`. Output is a
/// JSON object `{ project, endpoint, impact: [...] }`.
#[cfg(feature = "api-review")]
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct ApiImpactArgs {
    /// Project name (the multi-project isolation key).
    pub project: String,
    /// Endpoint path (e.g. `/api/users`) or name to analyse.
    pub endpoint: String,
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
}

/// Arguments for the `api-tool-map` subcommand (T008, v0.2.0).
///
/// Lists all `Tool` nodes (MCP tools) for a project joined with their handler
/// functions via `HANDLES` edges. Output is a JSON object
/// `{ project, tool_map: [...] }`.
#[cfg(feature = "api-review")]
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct ToolMapArgs {
    /// Project name (the multi-project isolation key).
    pub project: String,
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
}

/// Arguments for the `community` subcommand (T009, v0.2.0).
///
/// Runs Louvain modularity optimization on the project's `CALLS` graph and
/// prints the detected communities as a JSON object
/// `{ project, resolution, communities: [...] }`. Each community entry has
/// `id`, `members` (FQN list), `modularity` (Q_c contribution), and `size`.
#[cfg(feature = "community")]
#[derive(Parser, Debug, Clone, PartialEq)]
pub struct CommunityArgs {
    /// Project name (the multi-project isolation key).
    pub project: String,
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
    /// Louvain resolution parameter (γ). Higher values produce more, smaller
    /// communities; lower values produce fewer, larger communities. Default
    /// is `1.0` (standard Newman modularity). Must be > 0.
    #[arg(long)]
    pub resolution: Option<f64>,
}

/// Arguments for the `cross-service` subcommand (T010, v0.2.0).
///
/// Detects HTTP route patterns matching string literals in caller function
/// bodies and persists `CROSS_SERVICE_CALLS` edges. Output is a JSON object
/// `{ project, links: [...] }` where each link entry has `route_id`,
/// `route_pattern`, `caller_id`, `caller_file`, `caller_line`, and
/// `match_type` (`Exact` | `Parameterized` | `Wildcard`).
#[cfg(feature = "cross-service")]
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct CrossServiceArgs {
    /// Project name (the multi-project isolation key).
    pub project: String,
    /// Database path.
    #[arg(long, default_value = "./codenexus.lbug")]
    pub db: String,
}

/// Arguments for the `lsp-goto-def` subcommand (T007, v0.2.0).
///
/// Spawns `rust-analyzer` rooted at `--workspace` (default: current directory),
/// sends a `textDocument/definition` LSP request at `(file, line, col)`, and
/// prints the resolved [`lsp_types::Location`] as JSON to stdout. Returns exit
/// code 1 if `rust-analyzer` cannot be started or the query fails.
///
/// `line` and `col` are **0-based** per the LSP spec (matching the convention
/// of the [`LspProvider`](crate::lsp::LspProvider) trait).
#[cfg(feature = "lsp")]
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct LspGotoDefArgs {
    /// File path (absolute or relative to `--workspace`) of the symbol to query.
    pub file: String,
    /// 0-based line number (LSP `Position.line`).
    pub line: u32,
    /// 0-based column number (LSP `Position.character`).
    pub col: u32,
    /// Workspace root path for `rust-analyzer` (default: current directory).
    #[arg(long, default_value = ".")]
    pub workspace: String,
}

/// Arguments for the `lsp-hover` subcommand (T007, v0.2.0).
///
/// Spawns `rust-analyzer` rooted at `--workspace` (default: current directory),
/// sends a `textDocument/hover` LSP request at `(file, line, col)`, and prints
/// the resolved [`lsp_types::Hover`] as JSON to stdout. Returns exit code 1 if
/// `rust-analyzer` cannot be started or the query fails.
///
/// `line` and `col` are **0-based** per the LSP spec.
#[cfg(feature = "lsp")]
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct LspHoverArgs {
    /// File path (absolute or relative to `--workspace`) of the symbol to query.
    pub file: String,
    /// 0-based line number (LSP `Position.line`).
    pub line: u32,
    /// 0-based column number (LSP `Position.character`).
    pub col: u32,
    /// Workspace root path for `rust-analyzer` (default: current directory).
    #[arg(long, default_value = ".")]
    pub workspace: String,
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
            ram_first: false,
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
            uid: None,
            file: None,
            kind: None,
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
            uid: None,
            file: None,
            kind: None,
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
            uid: None,
            file: None,
            kind: None,
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
            ram_first: false,
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

    // --- Setup / Hook / Mcp (H13) ---

    #[test]
    fn cli_parses_setup_subcommand_defaults() {
        let cli = Cli::parse_from(["codenexus", "setup"]);
        match cli.command {
            Command::Setup(args) => assert!(!args.force),
            other => panic!("expected Setup, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_setup_with_force() {
        let cli = Cli::parse_from(["codenexus", "setup", "--force"]);
        match cli.command {
            Command::Setup(args) => assert!(args.force),
            other => panic!("expected Setup, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_hook_subcommand_defaults() {
        let cli = Cli::parse_from(["codenexus", "hook"]);
        match cli.command {
            Command::Hook(args) => assert_eq!(args.db, "./codenexus.lbug"),
            other => panic!("expected Hook, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_hook_with_db() {
        let cli = Cli::parse_from(["codenexus", "hook", "--db", "/tmp/x.lbug"]);
        match cli.command {
            Command::Hook(args) => assert_eq!(args.db, "/tmp/x.lbug"),
            other => panic!("expected Hook, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_mcp_subcommand_defaults() {
        let cli = Cli::parse_from(["codenexus", "mcp"]);
        match cli.command {
            Command::Mcp(args) => assert_eq!(args.db, "./codenexus.lbug"),
            other => panic!("expected Mcp, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_mcp_with_db() {
        let cli = Cli::parse_from(["codenexus", "mcp", "--db", "/tmp/x.lbug"]);
        match cli.command {
            Command::Mcp(args) => assert_eq!(args.db, "/tmp/x.lbug"),
            other => panic!("expected Mcp, got {other:?}"),
        }
    }

    #[test]
    fn setup_args_clone_eq() {
        let a = SetupArgs { force: true };
        assert_eq!(a, a.clone());
    }

    #[test]
    fn hook_args_clone_eq() {
        let a = HookArgs {
            db: "/tmp/x.lbug".into(),
        };
        assert_eq!(a, a.clone());
    }

    #[test]
    fn mcp_args_clone_eq() {
        let a = McpArgs {
            db: "/tmp/x.lbug".into(),
        };
        assert_eq!(a, a.clone());
    }

    // --- API Review subcommands (T008, v0.2.0) ---

    #[test]
    #[cfg(feature = "api-review")]
    fn cli_parses_api_route_map_subcommand_defaults() {
        let cli = Cli::parse_from(["codenexus", "api-route-map", "demo"]);
        match cli.command {
            Command::ApiRouteMap(args) => {
                assert_eq!(args.project, "demo");
                assert_eq!(args.db, "./codenexus.lbug");
            }
            other => panic!("expected ApiRouteMap, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "api-review")]
    fn cli_parses_api_route_map_with_db() {
        let cli = Cli::parse_from([
            "codenexus",
            "api-route-map",
            "demo",
            "--db",
            "/tmp/x.lbug",
        ]);
        match cli.command {
            Command::ApiRouteMap(args) => {
                assert_eq!(args.project, "demo");
                assert_eq!(args.db, "/tmp/x.lbug");
            }
            other => panic!("expected ApiRouteMap, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "api-review")]
    fn api_route_map_requires_project_arg() {
        let result = Cli::try_parse_from(["codenexus", "api-route-map"]);
        assert!(result.is_err(), "api-route-map without project should fail");
    }

    #[test]
    #[cfg(feature = "api-review")]
    fn cli_parses_api_shape_check_subcommand_defaults() {
        let cli = Cli::parse_from(["codenexus", "api-shape-check", "demo"]);
        match cli.command {
            Command::ApiShapeCheck(args) => {
                assert_eq!(args.project, "demo");
                assert_eq!(args.db, "./codenexus.lbug");
            }
            other => panic!("expected ApiShapeCheck, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "api-review")]
    fn cli_parses_api_shape_check_with_db() {
        let cli = Cli::parse_from([
            "codenexus",
            "api-shape-check",
            "demo",
            "--db",
            "/tmp/x.lbug",
        ]);
        match cli.command {
            Command::ApiShapeCheck(args) => {
                assert_eq!(args.project, "demo");
                assert_eq!(args.db, "/tmp/x.lbug");
            }
            other => panic!("expected ApiShapeCheck, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "api-review")]
    fn cli_parses_api_impact_subcommand_defaults() {
        let cli = Cli::parse_from([
            "codenexus",
            "api-impact",
            "demo",
            "/api/users",
        ]);
        match cli.command {
            Command::ApiImpact(args) => {
                assert_eq!(args.project, "demo");
                assert_eq!(args.endpoint, "/api/users");
                assert_eq!(args.db, "./codenexus.lbug");
            }
            other => panic!("expected ApiImpact, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "api-review")]
    fn cli_parses_api_impact_with_db() {
        let cli = Cli::parse_from([
            "codenexus",
            "api-impact",
            "demo",
            "/api/users",
            "--db",
            "/tmp/x.lbug",
        ]);
        match cli.command {
            Command::ApiImpact(args) => {
                assert_eq!(args.project, "demo");
                assert_eq!(args.endpoint, "/api/users");
                assert_eq!(args.db, "/tmp/x.lbug");
            }
            other => panic!("expected ApiImpact, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "api-review")]
    fn api_impact_requires_two_args() {
        let result = Cli::try_parse_from(["codenexus", "api-impact", "demo"]);
        assert!(result.is_err(), "api-impact without endpoint should fail");
    }

    #[test]
    #[cfg(feature = "api-review")]
    fn cli_parses_api_tool_map_subcommand_defaults() {
        let cli = Cli::parse_from(["codenexus", "api-tool-map", "demo"]);
        match cli.command {
            Command::ApiToolMap(args) => {
                assert_eq!(args.project, "demo");
                assert_eq!(args.db, "./codenexus.lbug");
            }
            other => panic!("expected ApiToolMap, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "api-review")]
    fn cli_parses_api_tool_map_with_db() {
        let cli = Cli::parse_from([
            "codenexus",
            "api-tool-map",
            "demo",
            "--db",
            "/tmp/x.lbug",
        ]);
        match cli.command {
            Command::ApiToolMap(args) => {
                assert_eq!(args.project, "demo");
                assert_eq!(args.db, "/tmp/x.lbug");
            }
            other => panic!("expected ApiToolMap, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "api-review")]
    fn route_map_args_clone_eq() {
        let a = RouteMapArgs {
            project: "demo".into(),
            db: "/tmp/x.lbug".into(),
        };
        assert_eq!(a, a.clone());
    }

    #[test]
    #[cfg(feature = "api-review")]
    fn shape_check_args_clone_eq() {
        let a = ShapeCheckArgs {
            project: "demo".into(),
            db: "/tmp/x.lbug".into(),
        };
        assert_eq!(a, a.clone());
    }

    #[test]
    #[cfg(feature = "api-review")]
    fn api_impact_args_clone_eq() {
        let a = ApiImpactArgs {
            project: "demo".into(),
            endpoint: "/api/users".into(),
            db: "/tmp/x.lbug".into(),
        };
        assert_eq!(a, a.clone());
    }

    #[test]
    #[cfg(feature = "api-review")]
    fn tool_map_args_clone_eq() {
        let a = ToolMapArgs {
            project: "demo".into(),
            db: "/tmp/x.lbug".into(),
        };
        assert_eq!(a, a.clone());
    }

    // --- Community (T009, v0.2.0) ---

    #[test]
    #[cfg(feature = "community")]
    fn cli_parses_community_subcommand_defaults() {
        let cli = Cli::parse_from(["codenexus", "community", "demo"]);
        match cli.command {
            Command::Community(args) => {
                assert_eq!(args.project, "demo");
                assert_eq!(args.db, "./codenexus.lbug");
                assert!(args.resolution.is_none());
            }
            other => panic!("expected Community, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "community")]
    fn cli_parses_community_with_db_and_resolution() {
        let cli = Cli::parse_from([
            "codenexus",
            "community",
            "demo",
            "--db",
            "/tmp/x.lbug",
            "--resolution",
            "2.5",
        ]);
        match cli.command {
            Command::Community(args) => {
                assert_eq!(args.project, "demo");
                assert_eq!(args.db, "/tmp/x.lbug");
                assert!((args.resolution.unwrap() - 2.5).abs() < f64::EPSILON);
            }
            other => panic!("expected Community, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "community")]
    fn community_requires_project_arg() {
        let result = Cli::try_parse_from(["codenexus", "community"]);
        assert!(result.is_err(), "community without project should fail");
    }

    #[test]
    #[cfg(feature = "community")]
    fn community_args_clone_eq() {
        let a = CommunityArgs {
            project: "demo".into(),
            db: "/tmp/x.lbug".into(),
            resolution: Some(2.0),
        };
        assert_eq!(a, a.clone());
    }

    // --- CrossService (T010, v0.2.0) ---

    #[test]
    #[cfg(feature = "cross-service")]
    fn cli_parses_cross_service_subcommand_defaults() {
        let cli = Cli::parse_from(["codenexus", "cross-service", "demo"]);
        match cli.command {
            Command::CrossService(args) => {
                assert_eq!(args.project, "demo");
                assert_eq!(args.db, "./codenexus.lbug");
            }
            other => panic!("expected CrossService, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "cross-service")]
    fn cli_parses_cross_service_with_db() {
        let cli = Cli::parse_from([
            "codenexus",
            "cross-service",
            "demo",
            "--db",
            "/tmp/x.lbug",
        ]);
        match cli.command {
            Command::CrossService(args) => {
                assert_eq!(args.project, "demo");
                assert_eq!(args.db, "/tmp/x.lbug");
            }
            other => panic!("expected CrossService, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "cross-service")]
    fn cross_service_requires_project_arg() {
        let result = Cli::try_parse_from(["codenexus", "cross-service"]);
        assert!(result.is_err(), "cross-service without project should fail");
    }

    #[test]
    #[cfg(feature = "cross-service")]
    fn cross_service_args_clone_eq() {
        let a = CrossServiceArgs {
            project: "demo".into(),
            db: "/tmp/x.lbug".into(),
        };
        assert_eq!(a, a.clone());
    }

    // --- LSP subcommands (T007, v0.2.0) ---

    #[test]
    #[cfg(feature = "lsp")]
    fn cli_parses_lsp_goto_def_subcommand_defaults() {
        let cli = Cli::parse_from([
            "codenexus",
            "lsp-goto-def",
            "/repo/src/main.rs",
            "10",
            "5",
        ]);
        match cli.command {
            Command::LspGotoDef(args) => {
                assert_eq!(args.file, "/repo/src/main.rs");
                assert_eq!(args.line, 10);
                assert_eq!(args.col, 5);
                assert_eq!(args.workspace, ".");
            }
            other => panic!("expected LspGotoDef, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "lsp")]
    fn cli_parses_lsp_goto_def_with_workspace() {
        let cli = Cli::parse_from([
            "codenexus",
            "lsp-goto-def",
            "/repo/src/main.rs",
            "0",
            "0",
            "--workspace",
            "/repo",
        ]);
        match cli.command {
            Command::LspGotoDef(args) => {
                assert_eq!(args.file, "/repo/src/main.rs");
                assert_eq!(args.line, 0);
                assert_eq!(args.col, 0);
                assert_eq!(args.workspace, "/repo");
            }
            other => panic!("expected LspGotoDef, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "lsp")]
    fn lsp_goto_def_requires_file_line_col() {
        let result = Cli::try_parse_from(["codenexus", "lsp-goto-def", "file.rs", "1"]);
        assert!(
            result.is_err(),
            "lsp-goto-def without col should fail"
        );
    }

    #[test]
    #[cfg(feature = "lsp")]
    fn cli_parses_lsp_hover_subcommand_defaults() {
        let cli = Cli::parse_from([
            "codenexus",
            "lsp-hover",
            "/repo/src/lib.rs",
            "3",
            "7",
        ]);
        match cli.command {
            Command::LspHover(args) => {
                assert_eq!(args.file, "/repo/src/lib.rs");
                assert_eq!(args.line, 3);
                assert_eq!(args.col, 7);
                assert_eq!(args.workspace, ".");
            }
            other => panic!("expected LspHover, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "lsp")]
    fn cli_parses_lsp_hover_with_workspace() {
        let cli = Cli::parse_from([
            "codenexus",
            "lsp-hover",
            "src/lib.rs",
            "0",
            "0",
            "--workspace",
            "/home/user/project",
        ]);
        match cli.command {
            Command::LspHover(args) => {
                assert_eq!(args.file, "src/lib.rs");
                assert_eq!(args.line, 0);
                assert_eq!(args.col, 0);
                assert_eq!(args.workspace, "/home/user/project");
            }
            other => panic!("expected LspHover, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "lsp")]
    fn lsp_hover_requires_three_args() {
        let result = Cli::try_parse_from(["codenexus", "lsp-hover", "file.rs"]);
        assert!(
            result.is_err(),
            "lsp-hover without line and col should fail"
        );
    }

    #[test]
    #[cfg(feature = "lsp")]
    fn lsp_goto_def_args_clone_eq() {
        let a = LspGotoDefArgs {
            file: "/r/x.rs".into(),
            line: 1,
            col: 2,
            workspace: "/r".into(),
        };
        assert_eq!(a, a.clone());
    }

    #[test]
    #[cfg(feature = "lsp")]
    fn lsp_hover_args_clone_eq() {
        let a = LspHoverArgs {
            file: "/r/x.rs".into(),
            line: 3,
            col: 4,
            workspace: ".".into(),
        };
        assert_eq!(a, a.clone());
    }

    // --- Complexity (T017, v0.2.1) ---

    #[test]
    #[cfg(feature = "complexity")]
    fn cli_parses_complexity_subcommand_defaults() {
        let cli = Cli::parse_from(["codenexus", "complexity", "demo"]);
        match cli.command {
            Command::Complexity(args) => {
                assert_eq!(args.project, "demo");
                assert_eq!(args.db, "./codenexus.lbug");
                assert!(!args.red_only);
                assert!(!args.sort_by_severity);
            }
            other => panic!("expected Complexity, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "complexity")]
    fn cli_parses_complexity_with_flags() {
        let cli = Cli::parse_from([
            "codenexus",
            "complexity",
            "demo",
            "--db",
            "/tmp/x.lbug",
            "--red-only",
            "--sort-by-severity",
        ]);
        match cli.command {
            Command::Complexity(args) => {
                assert_eq!(args.project, "demo");
                assert_eq!(args.db, "/tmp/x.lbug");
                assert!(args.red_only);
                assert!(args.sort_by_severity);
            }
            other => panic!("expected Complexity, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "complexity")]
    fn complexity_requires_project_arg() {
        let result = Cli::try_parse_from(["codenexus", "complexity"]);
        assert!(result.is_err(), "complexity without project should fail");
    }

    #[test]
    #[cfg(feature = "complexity")]
    fn complexity_args_clone_eq() {
        let a = ComplexityArgs {
            project: "demo".into(),
            db: "/tmp/x.lbug".into(),
            red_only: true,
            sort_by_severity: false,
            cyclomatic_green: None,
            cyclomatic_yellow: Some(5),
            cyclomatic_red: Some(8),
            cognitive_green: None,
            cognitive_yellow: None,
            cognitive_red: None,
            nesting_green: None,
            nesting_yellow: None,
            nesting_red: None,
            func_length_green: None,
            func_length_yellow: None,
            func_length_red: None,
            halstead_volume_green: None,
            halstead_volume_yellow: None,
            halstead_volume_red: None,
            maintainability_green: None,
            maintainability_yellow: None,
            maintainability_red: None,
            time_complexity_green: None,
            time_complexity_yellow: None,
            time_complexity_red: None,
            space_complexity_yellow: None,
            space_complexity_red: None,
        };
        assert_eq!(a, a.clone());
    }

    #[test]
    #[cfg(feature = "complexity")]
    fn cli_parses_complexity_with_threshold_flags() {
        let cli = Cli::parse_from([
            "codenexus",
            "complexity",
            "demo",
            "--cyclomatic-yellow",
            "5",
            "--cyclomatic-red",
            "8",
        ]);
        match cli.command {
            Command::Complexity(args) => {
                assert_eq!(args.project, "demo");
                assert_eq!(args.cyclomatic_yellow, Some(5));
                assert_eq!(args.cyclomatic_red, Some(8));
                // Untouched thresholds default to None.
                assert_eq!(args.cognitive_yellow, None);
                assert_eq!(args.cognitive_red, None);
                assert_eq!(args.time_complexity_yellow, None);
                assert_eq!(args.space_complexity_red, None);
            }
            other => panic!("expected Complexity, got {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "complexity")]
    fn cli_parses_complexity_with_enum_threshold_flags() {
        // TimeComplexity / SpaceComplexity use FromStr via clap value_parser.
        let cli = Cli::parse_from([
            "codenexus",
            "complexity",
            "demo",
            "--time-complexity-yellow",
            "O(n log n)",
            "--space-complexity-red",
            "O(n^2)",
        ]);
        match cli.command {
            Command::Complexity(args) => {
                assert_eq!(
                    args.time_complexity_yellow,
                    Some(crate::analysis::complexity::TimeComplexity::ONLogN)
                );
                assert_eq!(
                    args.space_complexity_red,
                    Some(crate::analysis::complexity::SpaceComplexity::ON2)
                );
            }
            other => panic!("expected Complexity, got {other:?}"),
        }
    }
}
