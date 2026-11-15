# ❓ CodeNexus FAQ

This page collects frequently asked questions about CodeNexus, grouped by topic. No answer found? Feel free to ask on [GitHub Issues](https://github.com/Kirky-X/codenexus/issues).

## 📋 Table of Contents

- [General](#-general)
- [Installation & build](#-installation--build)
- [Usage & commands](#️-usage--commands)
- [Parsing & language support](#-parsing--language-support)
- [Performance & memory](#-performance--memory)
- [Troubleshooting](#-troubleshooting)

---

## 🧭 General

### ❓ What is CodeNexus?

**CodeNexus** is a multi-language code knowledge graph tool. It indexes source repositories into a property graph:

| Capability | Notes |
|:-----------|:------|
| **Multi-language parsing** | tree-sitter, 22 languages in the default `full` preset |
| **Graph storage** | LadybugDB, 45 node types + 32 edge types |
| **Query & analysis** | Cypher-subset queries, call-chain/data-flow tracing, impact analysis, semantic search |
| **Agent integration** | MCP server (11 tools) + `setup` auto-onboarding for Claude Code / Cursor / Codex |

**Learn more**: the [User Guide](USER_GUIDE.md).

### ❓ What is CodeNexus good for?

| Scenario | Recommended commands |
|----------|----------------------|
| Onboarding onto an unfamiliar codebase, want to see its structure fast | `index` → `architecture` → `context --enhanced true` |
| Assessing the blast radius of a change | `detect_changes`, `impact` |
| Cleaning up uncalled functions | `dead_code` |
| API contract review | `route_map`, `shape_check`, `api_impact` |
| Making an AI agent understand the codebase | `setup` + `mcp` |
| Sharing indexes across the team | `export` / `import` |

### ❓ Is CodeNexus a library or a tool?

Both. `src/main.rs` is the CLI binary (`codenexus`), and `src/lib.rs` exposes the public API (`IndexFacade` / `QueryFacade` / `TraceFacade` and more) so you can embed it in your own program as a Rust crate — see the [📘 API Reference](API_REFERENCE.md).

---

## 📦 Installation & build

### ❓ Which Rust versions are supported?

The MSRV is **1.97.1** (`Cargo.toml` `rust-version`, synced with `clippy.toml` `msrv`). The CI toolchain is currently pinned to 1.97.1 (see `.github/workflows/ci.yml`). Formatting requires the nightly toolchain (`rustfmt.toml` uses nightly-only options).

### ❓ I only want some languages

Use feature presets or trim to a single language:

```bash
cargo build --release --no-default-features --features minimal   # Rust only
cargo build --release --no-default-features --features core      # C + Rust + Python
cargo build --release --no-default-features --features lang-c    # C only
```

The full feature list is in the [User Guide · Feature flags](USER_GUIDE.md#-build-presets-and-feature-flags).

### ❓ `cargo install codenexus` fails at link time

Two known environment issues:

1. **`undefined symbol: std::to_chars(..., _Float128, ...)`** (openEuler / CentOS and other GCC ≤ 12 systems): the prebuilt LadybugDB binary requires a recent `libstdc++`. Run `LBUG_BUILD_FROM_SOURCE=1 cargo install codenexus` to force a source build (requires `cmake`).
2. **mold linker reports `undefined symbol: __cpu_model`**: fixed since v0.3.7 (`build.rs` statically links `libgcc.a`); upgrade if you are on an older version.

### ❓ Where is the database file stored?

By default `.codenexus/<project>.lbug` (`<project>` comes from `--name`, or the `--path` directory name when absent); override with the global `--db`. Since v0.3.12, `--fresh true` deletes the old DB before indexing to reclaim dead space (repeated `--force` runs let dead space accumulate — there was once a case where 6.6 MB of source bloated into a 35 GB DB).

---

## 🛠️ Usage & commands

### ❓ Why must every argument be a long option?

The CLI is **strict flag style**: no positional arguments, parameters are snake_case long options, and booleans take explicit values. For example:

```bash
# ✗ Wrong: positional argument
codenexus query "MATCH (f:Function) RETURN f.name LIMIT 10"

# ✓ Correct
codenexus query --cypher "MATCH (f:Function) RETURN f.name LIMIT 10"

# ✓ Booleans take explicit values
codenexus index --path ./myrepo --name myrepo --ram_first true
```

Optional flags of frequently used commands **carry built-in defaults** (e.g. `trace --depth` defaults to 5, `search --limit` to 50, `impact --max_depth` to 0), so you never need placeholder flags like `--path_filter ""`; the full default list is in each command's `--help`. To pin parameters per project, use `.codenexus/config.json` (see the [User Guide · Configuration](USER_GUIDE.md#️-configuration-file)). The full conventions are in the [User Guide · Core conventions](USER_GUIDE.md#-core-conventions).

### ❓ Does `--project` take a project name or a project id?

Either. Every command with `--project` resolves via `resolve_project_id`: if it matches an existing project's `name`, the canonical `id` is used; otherwise it is treated as a raw project id. Only where explicitly documented may `--project ""` be passed to disable filtering.

### ❓ Are warnings on stderr normal?

Yes. All logs (including info level) go to stderr, and stdout carries only the command's own JSON output — redirecting `> out.json` needs no filtering. Benign stderr warnings include `inklog ... Failed to set log crate logger` and `storage::connection - skipping unsupported DDL statement`; hide them for human viewing with `2>/dev/null`. During indexing, per-phase progress lines like `[codenexus] [i/n] <phase> ok` also print to stderr. Log files are written to `.codenexus/logs/codenexus.log` (daily rotation, 30-day retention).

### ❓ How do I find out why a command failed and what the exit code means?

All errors funnel through `CodeNexusError` and map to stable exit codes: `0` success, `1` internal error, `2` invalid input / project not found / query error, `4` not found / database corrupt. Error output is a structured diagnostic receipt (stable rule code + evidence + fix wording). See the [API Reference · Error types](API_REFERENCE.md#-error-types--exit-codes).

### ❓ Which tools does MCP mode expose?

10: `query`, `trace`, `impact`, `search`, `context`, `architecture`, `diagram`, `arch_diff`, `dead_code`, `detect_changes`. They share the same `#[forge]` definitions as the CLI commands, and each tool description documents its parameter semantics and defaults; the server opens the database **read-only** and can coexist with `index`/`daemon` writers. Use `codenexus setup` to write MCP configs for Claude Code / Cursor / Codex automatically.

---

## 🔤 Parsing & language support

### ❓ Which languages are supported?

22 in the default `full` preset: C, Rust, Fortran, Python, TypeScript, Go, Java, C++, JavaScript, Ruby, Haskell, OCaml, Scala, PHP, C#, Bash, HTML, CSS, JSON, Regex, Verilog, Swift. For per-language extracted node/edge types, see the [README · Architecture section](../../README.md#️-architecture).

### ❓ Why does dead-code detection produce false positives?

tree-sitter does not expand procedural macros; there are three known blind spots:

| Blind spot | Symptom | Mitigation |
|------------|---------|------------|
| impl methods generated by `#[derive(...)]` | Reported as dead code | Known false positive; confirm manually |
| Macro entry points (`#[tokio::main]`, axum `#[route]`, etc.) | CALLS edges invisible | The `attribute_entries` config treats 6 attribute markers as entry points |
| `#[test]` / `#[rstest]` test functions | Macro invocations invisible | `test_patterns` name globs + `TEST_ATTRIBUTE_MARKERS` attribute recognition |

Also: calls to const fns in const context produce no CALLS edges; Bash's `trap cleanup EXIT` and string-concatenation indirect calls are reported at Medium confidence and need manual confirmation before deletion.

### ❓ Why doesn't route scanning find all axum routes?

`extract_axum_routes` (since v0.3.8) supports programmatic `Router::new().route("/x", get(h))`, but only recognizes bare-identifier handlers; path-qualified handlers (`get(crate::module::handler)`) and closure handlers are skipped and need manual review. Route strings registered purely by macros (macro arguments rather than standalone string literals) are invisible to `cross_service`.

### ❓ Are non-ASCII paths/project names supported?

Yes. Chinese/Japanese/Korean directories and file names, Unicode project names, and non-ASCII DB paths all have dedicated test coverage (`tests/non_ascii_path_test.rs`); the `i18n` feature (ICU4X case folding + NFC normalization) is included in the `full` preset.

---

## ⚡ Performance & memory

### ❓ Will indexing a large repo OOM?

v0.3.10–v0.3.12 shipped the L1–L7 seven-layer memory defense (memory budget, graph iterator views, streaming CSV, mpsc concurrency cap, adaptive degradation, pipeline streaming, buffer_pool cap), cutting index peak memory on a 70 GB host from ~60 GB to ~4 GB. See the [⚡ Performance Guide](PERFORMANCE.md).

### ❓ When should I use `--ram_first`?

RAM-first mode LZ4-compresses the source into memory and bulk-loads in a single pass, reducing LadybugDB write amplification. **Recommended for repos with ≥ 1 GB of source** (ADR-024); on the 1000-file comparison fixture ram-first actually uses more memory (568 MB vs 450 MB), which is expected behavior.

### ❓ Are query/trace performance guarantees provided?

Performance SLOs (Cypher query P99 ≤ 200 ms, trace P99 ≤ 500 ms; thresholds in the [`benches/README.md`](../../benches/README.md) SLO threshold table): the Criterion benchmark suite monitors them continuously (`query_bench`, `trace_bench`); measured baselines and reproduction steps are in the [⚡ Performance Guide](PERFORMANCE.md).

### ❓ Indexing keeps getting slower / the DB keeps growing

Repeated `--force` indexing lets dead space accumulate. `--fresh true` (v0.3.12) deletes the old DB file before indexing for a full reclaim; the read-only connection's `max_db_size` has been raised to a 4 TiB cap, so querying >16 GiB databases no longer crashes.

---

## 🔧 Troubleshooting

### ❓ `lsp_goto_def` / `lsp_hover` exit with code 2

`LSP communication error: server connection closed` usually means an environment/configuration problem where the language server process exits immediately on startup — not a tool bug. Confirm the LSP server (rust-analyzer, pyright, clangd, gopls, ts-lang-server, fortls, jdtls) is installed and can be started manually. LSP starts on demand: pure-Rust repos only start rust-analyzer.

### ❓ `query` fails without `--db`

Since v0.3.8: when `.codenexus/` contains exactly one `.lbug` file it is selected automatically; with multiple files you must pass `--db` explicitly.

### ❓ Symbol queries report `ambiguous symbol`

The ambiguity-resolution policy is "auto-select when there is exactly one match; error when it cannot be uniquely determined" (exit 2). Use a more precise symbol name (e.g. the qualified name), `--project` filtering, or the `context` command to narrow the scope.

### ❓ Cypher query errors or unsupported syntax?

Queries use a Cypher **subset** of LadybugDB: `UNION` / `UNION ALL` are supported (both branches must be valid on their own); label-disjunction expressions `WHERE (n:A OR n:B)` (labels belong in the `MATCH (n:A)` pattern) and write operations (`CREATE`/`DELETE`/`SET`/`MERGE`, which report `unsupported Cypher construct`) are not supported. Supported syntax is defined by what `query --cypher` actually accepts and by the [📘 API Reference](API_REFERENCE.md#queryfacade-query--search).

### ❓ How do I report a bug?

1. Collect: `codenexus --version`, Rust version, OS, the full command, the full error output, and a minimal reproduction repo.
2. File at [GitHub Issues](https://github.com/Kirky-X/codenexus/issues/new) (a Conventional Commits-style title helps triage).
3. Security vulnerabilities must **not** be filed publicly — use the GitHub [Security Advisories](https://github.com/Kirky-X/codenexus/security/policy) private disclosure channel first (fall back to email Kirky-X@outlook.com, handled by Kirky.X, only if unavailable); see the [🔒 Security doc](SECURITY.md).
