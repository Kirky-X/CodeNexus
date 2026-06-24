# CodeNexus CLI Skill

## Description

CodeNexus is a code knowledge graph indexing tool. It parses source code (C, Rust, Fortran, Python, TypeScript) using tree-sitter, builds a queryable graph in LadybugDB, and supports call-chain tracing, data-flow analysis, cross-language FFI tracking, and semantic search.

Use this Skill when you need to index a codebase, query its structure, trace function calls or data flow, analyze the impact of changes, or search for symbols.

## Prerequisites

Build the CLI first:

```bash
cargo build --release
```

The binary is at `target/release/codenexus`. For semantic search (optional), build with:

```bash
cargo build --release --features embed
```

## Commands

### index — Index a codebase

Indexes a codebase into the LadybugDB knowledge graph. Parses all supported source files, extracts symbols (functions, classes, variables, etc.), resolves call/data-flow/FFI relationships, and stores everything in the graph database.

```bash
codenexus index <PATH> --name <PROJECT_NAME> [OPTIONS]
```

**Options:**
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)
- `--force` — Re-parse every file, ignoring cached hashes
- `--lsp` — Enable LSP-enhanced extraction (reserved)
- `--embed` — Generate embeddings for semantic search (requires `embed` feature)

**Output (JSON):** `project_id`, `files_indexed`, `files_skipped`, `nodes_created`, `edges_created`, `duration_ms`

**Examples:**
```bash
# Index a Rust project
codenexus index /path/to/repo --name myproject

# Force full re-index
codenexus index /path/to/repo --name myproject --force

# Index with custom database location
codenexus index /path/to/repo --name myproject --db /tmp/graph.lbug

# Index with embeddings (requires embed feature)
codenexus index /path/to/repo --name myproject --embed
```

**Exit codes:** 0 success, 1 invalid input, 2 database locked, 3 system error, 4 database corrupt

---

### query — Execute a Cypher query

Runs a Cypher query against the graph database. Use this for custom graph queries not covered by the other commands.

```bash
codenexus query "<CYPHER>" [OPTIONS]
```

**Options:**
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)
- `--project <NAME>` — Optional project filter (informational)

**Output (JSON):** `columns`, `rows`, `duration_ms`

**Examples:**
```bash
# List all functions
codenexus query "MATCH (f:Function) RETURN f.name AS name, f.filePath AS file LIMIT 10"

# Count nodes by type
codenexus query "MATCH (n:Class) RETURN count(n) AS class_count"

# Find call relationships
codenexus query "MATCH (a:Function)-[:CALLS]->(b:Function) RETURN a.name, b.name LIMIT 20"

# Find functions in a specific file
codenexus query "MATCH (f:Function) WHERE f.filePath CONTAINS 'main.rs' RETURN f.name, f.startLine"
```

---

### trace — Trace a symbol's paths

Traces call chains and/or data-flow paths starting from a symbol. Supports depth-limited BFS traversal over `CALLS`, `FFI_CALLS`, `DATAFLOWS`, `READS`, and `WRITES` edges.

```bash
codenexus trace <SYMBOL> [OPTIONS]
```

**Options:**
- `--type <TYPE>` — Trace type: `calls`, `dataflow`, or `all` (default: `all`)
- `--depth <N>` — Maximum traversal depth (default: 3)
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

**Output (JSON):** `paths[].nodes`, `paths[].edges`, `paths[].depth`

**Examples:**
```bash
# Trace all paths from a function
codenexus trace main_function

# Trace only call chains, depth 5
codenexus trace parse_file --type calls --depth 5

# Trace only data flow
codenexus trace variable_x --type dataflow

# Use a specific database
codenexus trace main --type all --depth 2 --db /tmp/graph.lbug
```

---

### impact — Analyze impact radius

Analyzes the blast radius of changing a symbol. Performs reverse traversal to find all symbols that depend on the target.

```bash
codenexus impact <SYMBOL> [OPTIONS]
```

**Options:**
- `--depth <N>` — Maximum reverse-traversal depth (default: 3)
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

**Output (JSON):** List of affected symbols with their paths.

**Examples:**
```bash
# Analyze impact of changing a function
codenexus impact parse_file

# Deep impact analysis
codenexus impact critical_function --depth 10
```

---

### search — Search for symbols

Searches for symbols by name (structured search), content (BM25 full-text), or semantic meaning (vector similarity, requires `embed` feature).

```bash
codenexus search <TEXT> [OPTIONS]
```

**Options:**
- `--semantic` — Use semantic (vector) search (requires `embed` feature)
- `--limit <N>` — Maximum results (default: 10)
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

**Output (JSON):** Array of `{name, label, file_path, start_line, qualified_name, score}`

**Examples:**
```bash
# Search by name keyword
codenexus search parse

# Semantic search (requires embed feature)
codenexus search "解析函数" --semantic

# Limit results
codenexus search "database" --limit 5
```

---

### daemon — Start file-watching daemon

Watches a codebase directory and automatically triggers incremental indexing when code files change. Uses a configurable debounce window to batch consecutive changes.

```bash
codenexus daemon <PATH> --name <PROJECT_NAME> [OPTIONS]
```

**Options:**
- `--debounce-ms <MS>` — Debounce window in milliseconds (default: 2000)
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

**Behavior:**
- Watches the directory recursively
- Ignores non-code files (only `.c`, `.h`, `.rs`, `.f90`, `.f`, `.f95`, `.py`, `.ts`, `.tsx` trigger indexing)
- Debounces consecutive changes (default 2000ms)
- Pauses event processing during indexing
- Runs until interrupted (Ctrl+C)

**Examples:**
```bash
# Start daemon with default settings
codenexus daemon /path/to/repo --name myproject

# Custom debounce (500ms)
codenexus daemon /path/to/repo --name myproject --debounce-ms 500
```

---

### status — Show indexing status

Displays the indexing status for all projects in the database.

```bash
codenexus status [OPTIONS]
```

**Options:**
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

**Output (JSON):** List of projects with their indexing metadata.

**Examples:**
```bash
codenexus status
codenexus status --db /tmp/graph.lbug
```

---

### list — List indexed projects

Lists all projects that have been indexed in the database.

```bash
codenexus list [OPTIONS]
```

**Options:**
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

**Output (JSON):** Array of project names and metadata.

**Examples:**
```bash
codenexus list
codenexus list --db /tmp/graph.lbug
```

---

### clean — Remove a project's index

Removes a project and all its associated nodes and edges from the database.

```bash
codenexus clean <PROJECT_NAME> [OPTIONS]
```

**Options:**
- `--db <DB_PATH>` — Database path (default: `./codenexus.lbug`)

**Examples:**
```bash
codenexus clean myproject
codenexus clean old_project --db /tmp/graph.lbug
```

---

## Typical Workflows

### Workflow 1: Index and explore a new codebase

```bash
# 1. Index the codebase
codenexus index /path/to/repo --name myproject

# 2. List all functions
codenexus query "MATCH (f:Function) RETURN f.name, f.filePath, f.startLine ORDER BY f.name LIMIT 50"

# 3. Search for a specific symbol
codenexus search "parse"

# 4. Trace a function's call chain
codenexus trace main --type calls --depth 5

# 5. Check what would be affected by changing a function
codenexus impact critical_function --depth 5
```

### Workflow 2: Continuous indexing with daemon

```bash
# 1. Initial index
codenexus index /path/to/repo --name myproject

# 2. Start daemon for continuous updates
codenexus daemon /path/to/repo --name myproject --debounce-ms 1000

# 3. In another terminal, query the always-up-to-date graph
codenexus query "MATCH (f:Function)-[:CALLS]->(g:Function) RETURN f.name, g.name LIMIT 20"
```

### Workflow 3: Multi-project management

```bash
# Index multiple projects into the same database
codenexus index /path/to/project-a --name projectA --db /shared/graph.lbug
codenexus index /path/to/project-b --name projectB --db /shared/graph.lbug

# List all projects
codenexus list --db /shared/graph.lbug

# Query a specific project's functions
codenexus query "MATCH (f:Function) WHERE f.project = 'projectA' RETURN f.name LIMIT 10" --db /shared/graph.lbug

# Clean up a project
codenexus clean projectA --db /shared/graph.lbug
```

### Workflow 4: Cross-language FFI tracing

```bash
# Index a mixed Rust/C codebase
codenexus index /path/to/mixed-repo --name ffiproject

# Trace from a Rust function through FFI to C
codenexus trace rust_entry_point --type calls --depth 10

# Query FFI call edges
codenexus query "MATCH (a:Function)-[:FFI_CALLS]->(b:Function) RETURN a.name, b.name, a.filePath, b.filePath"
```

## Supported Languages

| Language | Extensions | Key Extractions |
|----------|-----------|-----------------|
| C | `.c`, `.h` | Functions, calls, `#include`, typedef, globals |
| Rust | `.rs` | `fn`, `struct`, `enum`, `trait`, `impl`, `extern "C"`, `use` |
| Fortran | `.f90`, `.f`, `.f95` | `subroutine`, `function`, `module`, `ISO_C_BINDING`, `call` |
| Python | `.py` | `def`, `class`, `import`, `__init__.py` |
| TypeScript | `.ts`, `.tsx` | `function`, `class`, `import`, `export` |

## Node Types (20)

Project, Folder, File, Module, Class, Struct, Enum, Trait, Impl, Function, Method, Variable, GlobalVar, Parameter, Const, Static, Macro, TypeAlias, Typedef, Namespace

## Edge Types (14)

CONTAINS, DEFINES, MEMBER_OF, CALLS, FFI_CALLS, DATAFLOWS, READS, WRITES, IMPLEMENTS, EXTENDS, USES_TYPE, REFERENCES, IMPORTS, INCLUDES

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Invalid input (path not found, bad arguments) |
| 2 | Database locked (retry) |
| 3 | System error (out of memory) |
| 4 | Database corrupt |
