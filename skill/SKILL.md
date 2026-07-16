---
name: codenexus
description: "Code knowledge graph indexer and query tool. Use when indexing code, tracing calls, analyzing impact, or querying the graph. Triggers: index codebase, trace calls, impact analysis, codenexus. when_to_use: Triggers include 'index a codebase', 'query the graph', 'trace calls from X', 'what's the impact of changing X', 'search for symbol', 'start the daemon', 'list projects', 'clean project', 'export/import graph', 'show context of X', 'what changed', 'rename X', 'set up MCP', or any reference to CodeNexus / codenexus CLI."
---

# CodeNexus CLI Skill

> **Verified against `codenexus 0.4.2`.** The CLI is **strictly flag-based**: there are **no positional arguments**, and almost every function parameter is a **mandatory flag** (plain `String`/`u32`/`bool` types in the source map to required flags — only `--db <DB_PATH>` and `--debounce-ms <MS>` are global options with defaults). Booleans are pass-by-value (e.g. `--force true`, `--apply true`, `--cross_service false`).

## Description

CodeNexus is a code knowledge graph indexing tool. It parses source code (C, Rust, Fortran, Python, TypeScript, Go, Java, C++) using tree-sitter, builds a queryable graph in LadybugDB, and supports call-chain tracing, data-flow analysis, cross-language FFI tracking, semantic search, change-impact analysis, refactoring proposals, LSP integration, and MCP server integration.

Use this Skill when you need to index a codebase, query its structure, trace function calls or data flow, analyze the impact of changes, search for symbols, watch files for incremental updates, manage projects, export/import graph artifacts, inspect a symbol's 360° context, detect symbols affected by git changes, propose renames, query an LSP server, or set up MCP integration with AI agents.

## Conventions (apply to every subcommand)

- **No positional arguments.** Everything is a named flag. `codenexus query "MATCH ..."` fails; use `codenexus query --cypher "MATCH ..."`.
- **Booleans take a value**: `--force true`, `--apply true`, `--cross_service false`, `--embed false`, `--ram_first false`, `--enhanced false`.
- **Project filter accepts BOTH name and id.** All commands that take `--project <VALUE>` resolve the value via `resolve_project_id` (in `src/service/project.rs`): if the value matches a stored project `name`, the canonical project `id` is used; otherwise the value is treated as a raw project id. Pass `--project ""` (empty string) only where explicitly documented to disable the filter.
- **Plain-typed params are required.** Function parameters with plain `String`/`u32`/`bool` types (no `Option<T>`) map to **mandatory** CLI flags with no built-in defaults. Empty strings are allowed where the source explicitly handles them (e.g. `--edge_types ""`, `--path_filter ""`, `--protocol ""`).
- **Global options** on every command: `--db <DB_PATH>` and `--debounce-ms <MS>` (default `2000`, daemon-only relevance — safe to ignore otherwise).
- **Default DB path** (when `--db` is omitted): `.codenexus/<project>.lbug`, where `<project>` is sanitized from the subcommand's `--name` arg (preferred), or the dirname of its `--path` arg, or the fallback `codenexus`. The `.codenexus/` directory is auto-created. Example: `codenexus index --path /home/me/myrepo --name myrepo ...` resolves to `.codenexus/myrepo.lbug`. To override, pass `--db /custom/path.lbug`.
- **Stderr noise**: every connection prints warnings like `inklog ... Failed to set log crate logger` and `storage::connection - skipping unsupported DDL statement`. These are benign; filter with `2>/dev/null` to see clean JSON, or `2>&1 | grep -v "WARN\|skipping unsupported"`.
- **JSON output**: every command that returns a result prints a single JSON object/array to stdout (commands like `daemon`, `hook`, `mcp` are streaming/long-running and do not emit a final JSON blob).

## Prerequisites

Install from crates.io (recommended):

```bash
cargo install codenexus
```

Or build from source:

```bash
git clone https://github.com/Kirky-X/codenexus.git
cd codenexus
cargo build --release
```

The binary is at `target/release/codenexus` (or `~/.cargo/bin/codenexus` if installed via `cargo install`). For semantic search (optional), install with:

```bash
cargo install codenexus --features embed
```

Feature presets: `minimal` (Rust only), `core` (C+Rust+Python), `full` (all 8 languages + daemon + analysis + complexity + api-review + community + cross-service + lsp + mcp). The `mcp` feature enables the sdforge-based MCP server (`codenexus mcp`). At least one `lang-*` feature is required — the crate fails to compile otherwise.

## Command Quick Reference

CodeNexus has **28 subcommands** grouped into eight functional areas. **All required flags and detailed output schemas are documented in [`references/commands.md`](references/commands.md).** Run `codenexus <command> --help` for the auto-generated flag list.

| Command | Area | One-line description |
|---------|------|----------------------|
| `index` | Indexing | Parse a codebase with tree-sitter and load nodes/edges into LadybugDB. |
| `list` | Indexing | List indexed projects (`id`, `name`, `root_path`, `file_count`, `indexed_at`, `last_commit`). |
| `status` | Indexing | Show indexing status with a `stale` flag when the working tree has moved past `last_commit`. |
| `clean` | Indexing | Remove a project and all its nodes/edges. Accepts `--project <name_or_id>`. |
| `daemon` | Indexing | Watch a directory recursively and incrementally re-index on file change. Long-running. |
| `query` | Query | Execute a Cypher query. ADR-021 validator rejects destructive clauses at the CLI boundary. |
| `search` | Query | Symbol search (exact/regex/fuzzy/graph/multi) or BM25 full-text. ⚠️ See Known Issues — `query` is the reliable path. |
| `trace` | Tracing | Trace a symbol's call/dataflow paths with optional cycle detection and cross-service traversal. |
| `impact` | Tracing | Reverse BFS to find all symbols that depend on a target. Narrow `--edge_types` + `--max_depth` on large graphs. |
| `context` | Tracing | 360° view of a symbol: callers/callees/processes/routes/endpoints. `--enhanced false` is stable in 0.4.2. |
| `dead_code` | Analysis | Detect unreferenced `Function`/`Method` nodes with confidence levels (High/Medium/Low). |
| `architecture` | Analysis | High-level overview: module boundaries, dependency directions, layers, entry points, hotspots. |
| `complexity` | Analysis | AST-based per-function complexity (cyclomatic/cognitive/nesting/length/Halstead/MI/time/space). 26 threshold flags required. |
| `community` | Analysis | Louvain community detection on the call graph. |
| `cross_service` | Analysis | Detect cross-service call links (HTTP REST, gRPC, GraphQL, message queue, event bus). |
| `route_map` | Analysis | List HTTP API routes and their handler functions. |
| `tool_map` | Analysis | List MCP tool definitions and their handler functions. |
| `shape_check` | Analysis | Validate API endpoint shape consistency (pagination envelope, error shape). |
| `api_impact` | Analysis | Trace which callers would be affected by a given API endpoint change. |
| `detect_changes` | Refactoring | Run `git diff` and map touched files/lines to indexed symbols with risk classification. |
| `rename` | Refactoring | Propose graph + text edits for renaming a symbol. Dry-run by default; `--apply true` writes to disk. |
| `lsp_goto_def` | LSP | Query LSP Go-to-Definition at a file/line/col. Auto-detects language server. |
| `lsp_hover` | LSP | Query LSP Hover info at a file/line/col. Auto-detects language server. |
| `export` | Team | Dump the LadybugDB database to a zstd-compressed artifact with a JSON manifest. |
| `import` | Team | Decompress a team artifact into LadybugDB. Optionally triggers an incremental reindex. |
| `setup` | MCP | Auto-detect installed AI agents (Claude Code, Cursor, Codex) and write MCP config for `codenexus mcp`. |
| `hook` | MCP | Emit PreToolUse/PostToolUse hook JSON. Always exits 0; never blocks. Long-running (reads stdin). |
| `mcp` | MCP | Serve MCP tools (`query`/`trace`/`impact`/`search`/`context`) over stdio. Long-running. Requires `mcp` feature. |

## Critical Known Issues (0.4.2)

The full list with severities is in [`references/appendix.md`](references/appendix.md#known-issues-in-042). The ones most likely to bite during normal use:

- **`search` returns `{"count":0,"results":[]}` against default indexes** — name-search tables are not populated by the default indexing flow. Use `query` (e.g. `MATCH (f:Function) WHERE f.name CONTAINS 'parse' RETURN ...`) as the reliable symbol lookup path.
- **`context --enhanced true` may raise a generic error** against some projects. Use `--enhanced false` until fixed.
- **`rename` dry-run `graph_edit.new_qualified_name` may still show the old name**, and ambiguous same-named symbols are silently resolved to one match. Verify proposed edits carefully before `--apply true`.
- **`export` manifest hardcodes `"codenexus_version":"0.3.4"`** regardless of binary version (cosmetic only).
- **Some failure paths exit 0** (e.g. `lsp_*` when no language server is running, `list --db <missing>` returns `[]`). CI scripts must inspect the JSON payload, not rely solely on the exit code.
- **Rust call graph is a lower bound** — trait-object `dyn` dispatch and many cross-module calls are not captured by tree-sitter. Treat `dead_code`/`trace`/`impact` results for Rust as a triage list, not ground truth.
- **`impact` on large graphs can exceed the 120s tool timeout** with broad `--edge_types`/`--max_depth` and a high-degree target. Narrow edge types, lower `--max_depth`, and pick a leaf target.

## References

Detailed documentation is split into supporting files that load on demand. The main SKILL.md intentionally stays short — every line is a recurring token cost once the skill is active.

| Reference | Contents |
|-----------|----------|
| [`references/commands.md`](references/commands.md) | Full flag list, options, output schemas, and notes for all 28 subcommands. |
| [`references/storage-model.md`](references/storage-model.md) | `CodeRelation` NODE TABLE design, 44 node types, 24 edge types, confidence tiers. |
| [`references/workflows.md`](references/workflows.md) | Nine end-to-end workflows: indexing, daemon, multi-project, FFI, refactoring, team artifacts, MCP, complexity audit, API surface. |
| [`references/appendix.md`](references/appendix.md) | Supported languages (8), exit codes, full known-issues table, example programs. |

## Storage Model (essential for `query` users)

CodeNexus stores edges as **nodes** in a `CodeRelation` NODE TABLE, not as LadybugDB REL relationships. **`MATCH ()-[r]->()` and `MATCH ()-[r:CALLS]->()` return 0** — there are no REL relationships. To traverse edges, query the `CodeRelation` node table:

```cypher
-- Count edges by type
MATCH (r:CodeRelation) WHERE r.type='CALLS' RETURN count(r)

-- Find callers of a symbol (reverse traversal)
MATCH (r:CodeRelation) WHERE r.type='CALLS' AND r.target='my_namespace::my_func'
RETURN r.source, r.filePath

-- Find callees of a symbol (forward traversal)
MATCH (r:CodeRelation) WHERE r.type='CALLS' AND r.source='my_namespace::my_func'
RETURN r.target, r.filePath
```

`CodeRelation` columns: `id`, `source`, `target`, `type`, `confidence`, `confidenceTier`, `reason`, `startLine`, `project`. `source`/`target` hold the symbol `qualifiedName` (equal to `Function.id`). High-level commands (`trace`, `impact`, `dead_code`, `architecture`, `community`, etc.) abstract over this layout — they read `CodeRelation` internally so you don't have to write the join yourself. Full schema (44 node types, 24 edge types) is in [`references/storage-model.md`](references/storage-model.md).
