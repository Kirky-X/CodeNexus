# Appendix Reference

> Supported languages, exit codes, known issues, and example programs. Part of the CodeNexus skill — see [SKILL.md](../SKILL.md) for the overview.

## Supported Languages

| Language | Extensions | Key extractions |
|----------|-----------|-----------------|
| C | `.c`, `.h` | Functions, calls, `#include`, typedef, globals |
| Rust | `.rs` | `fn`, `struct`, `enum`, `trait`, `impl`, `extern "C"`, `use` |
| Fortran | `.f90`, `.f`, `.f95` | `subroutine`, `function`, `module`, `ISO_C_BINDING`, `call` |
| Python | `.py` | `def`, `class`, `import`, `__init__.py` |
| TypeScript | `.ts`, `.tsx` | `function`, `class`, `import`, `export` |
| Go | `.go` | `func`, `struct`, `interface`, `import` |
| Java | `.java` | `class`, `method`, `import`, `package` |
| C++ | `.cpp`, `.cc`, `.cxx`, `.c++`, `.hpp`, `.hh`, `.hxx`, `.h++` | `class`, `function`, `#include`, `namespace`, `template` |

> Static analysis limitation: For Rust in particular, trait-object `dyn` dispatch and many cross-module calls are not captured by tree-sitter. The resulting call graph is a lower bound. The reserved `--lsp true` flag and the `lsp_goto_def`/`lsp_hover` commands point at the future LSP-augmented path.

## Exit Codes

Exit codes are produced by `CodeNexusError::exit_code()` in `src/service/error.rs` (with `IndexError::exit_code()` for index-flow errors). The mapping is:

| Code | Meaning | Source variants |
|------|---------|-----------------|
| 0 | Success (also: non-fatal parse errors that do not abort indexing) | `IndexError::Parse(_)` |
| 1 | Internal / system error (IO, JSON serialization, Kit, daemon, cache, embed, LSP, discover) | `Internal`, `Io`, `Json`, `Discover`, `Daemon`, `Cache`, `Embed`, `Lsp`, `IndexError::PathNotFound`, `IndexError::Io`, `IndexError::Discover` |
| 2 | Client / database error (invalid input, project not found, query/trace/storage errors, DB locked, resolve/phase errors) | `InvalidInput`, `ProjectNotFound`, `Query`, `Trace`, `Storage`, `Resolve`, `Phase`, `IndexError::DatabaseLocked`, `IndexError::Storage` |
| 3 | (reserved, currently unused) | — |
| 4 | Not found / database corrupt | `NotFound`, `IndexError::DatabaseCorrupt` |

> ✅ **Fixed (Problem E):** all failure paths now exit non-zero in 0.3.4 — no-subcommand, symbol-not-found, ambiguous-symbol, `lsp_hover`/`lsp_goto_def` with no language server (exit 2: `failed to start LSP server`), and `list --db <missing path>` (exit 4: NotFound, with a clear message instead of returning `[]`). CI scripts can now rely on the exit code for these commands. (Verified 2026-07-16.)

## Known Issues

Originally recorded in `temp/problem.md` against a `--version` polluted by the `sdforge` dependency (it reported `0.4.2`); the binary is in fact `0.3.5` per `Cargo.toml`. Current status (verified 2026-07-16 on a self-indexed CodeNexus graph):

| ID | Severity | Command(s) affected | Summary |
|----|----------|---------------------|---------|
| A | P0 (fixed) | All analysis commands incl. `context --enhanced true` | `--project` resolves name → id via `resolve_project_id` (`src/service/project.rs`). `context --enhanced true` is now wired in too. Verified: `context --enhanced true --project CodeNexus` (name) returns a full `SymbolContext`. |
| B | P1 (fixed) | `complexity` | `Function.content` is empty after `index`; `complexity` falls back to reading source from disk via Project `rootPath` + `filePath` + line range. |
| C | P2 (fixed) | `rename` | Dry-run `new_qualified_name` recomputes the `.`/`#` suffix; ambiguous symbols fail fast (`ambiguous 'new': 99 candidates`, exit 2, ~0.9s) instead of silently resolving to one match or timing out. |
| D | P2 (fixed) | `export`/`import` | Manifest version read via `env!("CARGO_PKG_VERSION")` (`src/service/export.rs`); matches the binary. The `0.4.2` seen earlier was `sdforge` polluting `--version`, not a codenexus release. |
| E | P3 (fixed) | `lsp_*`, `list`, `complexity` | No-subcommand / not-found / ambiguous exit non-zero. `lsp_*` (no server) exits 2 (`failed to start LSP server`); `list --db <missing>` exits 4 (NotFound). Verified 2026-07-16. |
| F | P3 (fixed) | `context --enhanced true` | Resolves `--project <name\|id>`; ambiguous symbols fail fast (exit 1, ~0.3s) instead of raising a generic error or timing out. |
| Search | P2 (fixed) | `search` | The empty-`project` filter (appended `AND n.project = ''`, dropping every row) and the silent per-table `Err(_) => continue` (swallowing storage errors) were fixed in `src/query/structured.rs`. Verified: `search --text parse` returns `count:3`. |
| Impact large graph | P1 (fixed) | `impact` | The ~77s N+1 regression on large subgraphs (per-id × per-label traversal in phase-3 node materialization, ~85k DB round-trips) is fixed in 0.3.4: `load_graph` BFS now caps at `MAX_SUBGRAPH_NODES=5000` (raised from 1000 in v0.3.7 after bulwark testing showed high-fanin symbols hit the first-hop cap) and sets `truncated:true` on the cap; node materialization is batched (one `WHERE id IN [...]` per label, ~17 round-trips). Verified 2026-07-17 (6289 functions): `sanitize_project_name --depth 3` → `node_count=1000, truncated:true, ~5s` (was ~77s / 5034 nodes). |
| Concurrent read | P1 (fixed) | `query`, `list`, `search`, `impact`, `context`, `trace` | These read-only commands now open the DB via `StorageConnection::open_read_only`, propagated through the Storage, Query, and Trace modules + Kit bootstrap (`KitBootstrapConfig.read_only` → each module's `Config.read_only`). Multiple processes read the same file DB concurrently (DuckDB/LadybugDB shared-read) without contending on the write lock; read-only skips schema init. Concurrent writers still take an exclusive file lock — a second writer fails fast with exit 2 and a clear message naming the conflicting PID (`StorageError::DatabaseLocked` → `extract_db_locked_hint`). Verified 2026-07-17: 6 concurrent `query` procs → 6/6 exit 0; an `index` holding the write lock + a read-only `query` → query exit 0 (MVCC, readers not blocked); two `index` procs competing → second exits 2 with the PID. |
| Rust call graph | P1 (open) | `dead_code`, `trace`, `impact` (Rust) | Trait `dyn` dispatch and cross-module calls are not captured; treat results as a lower bound. |

## Examples

Runnable example programs live in `examples/src/bin/`:

| Example | Covers |
|---------|--------|
| `basic_indexing` | `index` + Cypher query |
| `cypher_query` | `query` with multiple Cypher patterns |
| `symbol_search` | `search` with empty-result handling |
| `call_tracing` | `trace --type calls` + graph construction |
| `impact_analysis` | `impact` reverse BFS |
| `export_import` | DB copy (note: production uses `codenexus export`/`import` with zstd + manifest) |

Run with: `cargo run --manifest-path examples/Cargo.toml --bin <name>`

> The examples use `IndexFacade`/`QueryFacade`/`TraceFacade` directly rather than the Kit registry, because Kit creates per-subsystem connections that can cause file-DB visibility issues. Direct Facade use is the recommended programmatic API.
