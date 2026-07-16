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

> ⚠️ **Known issue (Problem E):** Some failure paths still exit 0 in 0.4.2 (e.g. `lsp_hover`/`lsp_goto_def` when no language server is running, and `list --db <missing path>` which returns `[]`). Scripts in CI must inspect the JSON payload, not rely solely on the exit code, for these commands.

## Known Issues in 0.4.2

Tracked in `temp/problem.md`:

| ID | Severity | Command(s) affected | Summary |
|----|----------|---------------------|---------|
| A | P0 (fixed) | All analysis commands | `--project` now resolves name → id via `resolve_project_id` (`src/service/project.rs`). |
| B | P1 (fixed) | `complexity` | `Function.content` is empty after `index`; `complexity` falls back to reading source from disk via Project `rootPath` + `filePath` + line range. |
| C | P2 | `rename` | Dry-run `graph_edit.new_qualified_name` may still show the old name; ambiguous same-named symbols are silently resolved to one match. |
| D | P2 | `export`/`import` | Export manifest hardcodes `"codenexus_version":"0.3.4"` regardless of binary version. |
| E | P3 | `lsp_*`, `list`, `complexity` | Some failure paths still exit 0. CI scripts must inspect JSON. |
| F | P3 | `context --enhanced true` | Raises a generic `CodeNexus error occurred` against some projects. Use `--enhanced false` until fixed. |
| Search | P2 | `search` | Returns `{"count":0,"results":[]}` against default indexes. Use `query` for symbol lookup. |
| Rust call graph | P1 | `dead_code`, `trace`, `impact` (Rust) | Trait `dyn` dispatch and cross-module calls are not captured; treat results as a lower bound. |

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
