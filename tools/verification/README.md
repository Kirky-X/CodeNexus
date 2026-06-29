# CodeNexus Cross-Validation Verification Tools

> Cross-validate CodeNexus parsing against gitnexus reference indexes across 5 languages and 8 sample repositories.

## Prerequisites

1. **CodeNexus** — built from this repo:
   ```sh
   cargo build --bin codenexus
   ```

2. **gitnexus** — installed and indexes built for all sample repos:
   ```sh
   npx gitnexus analyze --repo /path/to/repo
   ```
   See `samples.json` for the list of repos and their `gitnexus_name` mappings.

3. **Sample repos** — cloned and checked out to the commits in `samples.json`:
   ```sh
   ./fetch_samples.sh
   ```

## Quick Start

### Single sample

```sh
# Index a repo with CodeNexus, fetch gitnexus reference, compare, report
./target/debug/codenexus-verify single \
  --repo /home/dev/projects/velo \
  --name velo \
  --language rust
```

Outputs (in `tools/verification/results/`):
- `<name>.codenexus.json` — CodeNexus node/edge counts by type
- `<name>.gitnexus.json` — gitnexus node/edge counts by label/type
- `<name>.report.md` — Markdown diff report with severity ratings

### Batch (all samples)

```sh
./target/debug/codenexus-verify batch --corpus tools/verification/samples.json
```

Generates `_aggregate.report.md` summarizing all samples.

> **Note**: Batch mode re-indexes each sample into `./codenexus.lbug`. For large corpora, use `--resume` to skip samples with existing JSON results (but note: query comparisons require the DB to contain the current sample's data, so `--resume` may produce stale query diffs).

### Resume (skip re-indexing)

```sh
./target/debug/codenexus-verify single --repo ... --name velo --language rust --resume
```

Loads existing `<name>.codenexus.json` instead of re-indexing. The gitnexus reference is always re-fetched.

## Report Fields

### Summary Table

| Field | Description |
|-------|-------------|
| Overall | PASS (all queries match) / FAIL (any critical diff) |
| Critical discrepancies | Count of 100% deltas or query set mismatches |
| Major discrepancies | Count of >10% deltas on comparable types |
| Minor discrepancies | Count of ≤10% deltas on comparable types |

### Node Type Comparison

Compares node counts for types that both CodeNexus and gitnexus index (per `type_map.json`). Types marked `codenexus_only` or `gitnexus_only` are excluded.

### Edge Type Comparison

Same as node types, but for `CodeRelation` edge types. CodeNexus stores edges in a single `CodeRelation` table with a `type` column.

### Query Comparison

Runs 8 Cypher queries (from `queries/*.cql`) against both sides and compares the result sets (order-insensitive). Each `.cql` file contains both a CodeNexus version and a gitnexus version, separated by comments.

| Query | Tests |
|-------|-------|
| `callers_of_function` | CALLS edge reverse resolution |
| `callees_of_function` | CALLS edge forward resolution |
| `class_methods` | Class→Method containment |
| `extends_chain` | EXTENDS edge traversal |
| `implements_list` | IMPLEMENTS edge traversal |
| `imports_of_file` | IMPORTS edge traversal |
| `file_contains_symbols` | DEFINES edge resolution |
| `function_count_by_file` | Per-file function distribution |

## Type Mapping

`type_map.json` defines the canonical type mappings between CodeNexus and gitnexus:

- **`comparable`** — both sides index this type; counts are compared directly
- **`codenexus_only`** — only CodeNexus indexes this (e.g. `Parameter`, `Variable`)
- **`gitnexus_only`** — only gitnexus indexes this (e.g. `Folder`, `Community`, `Process`)

If a type is miscategorized (e.g. CodeNexus models it differently but it's marked `comparable`), the report will show a false 100% critical diff. See `triage.md` for known design differences.

## Known Limitations

1. **CSV escaping bug** (B6 in triage.md): LadybugDB's COPY parser cannot handle RFC 4180 quoted fields containing backslashes or certain quotes. This blocks indexing of C projects with macros (redis) and mixed-language projects with Python method signatures (subno.ts). **Fix pending.**

2. **Query sampling noise** (B5 in triage.md): All 8 queries use `LIMIT 200` without deterministic `ORDER BY`, producing false set diffs. **Fix pending.**

3. **No per-project DB isolation in batch mode**: The batch mode re-indexes all samples into the same `./codenexus.lbug` DB. Query comparisons may return cross-project results. **Workaround: run each sample individually with a fresh DB.**

4. **CodeNexus `CodeRelation` is a NODE TABLE, not a REL TABLE**: Queries must use `MATCH (r:CodeRelation) WHERE r.source = ... AND r.type = ...` instead of `MATCH ()-[r:CodeRelation]->()`. The `.cql` files handle this via separate CodeNexus/gitnexus query sections.

5. **gitnexus analysis artifacts excluded**: `Process`, `Community`, `Route`, `Tool` nodes and `STEP_IN_PROCESS`/`ENTRY_POINT_OF`/`HANDLES_ROUTE` edges are gitnexus-specific analysis features. They appear in the "Analysis Artifacts" section of each report but are excluded from the comparison.

## File Layout

```
tools/verification/
├── README.md              — this file
├── samples.json           — 8 sample repos (name, language, path, commit)
├── type_map.json          — CodeNexus ↔ gitnexus type mappings
├── fetch_samples.sh       — clone/checkout sample repos
├── queries/               — 8 Cypher query files (.cql)
│   ├── callers_of_function.cql
│   ├── callees_of_function.cql
│   ├── class_methods.cql
│   ├── extends_chain.cql
│   ├── file_contains_symbols.cql
│   ├── function_count_by_file.cql
│   ├── implements_list.cql
│   └── imports_of_file.cql
├── src/                   — verifier source
│   ├── main.rs            — CLI entry point (single/batch/fetch-samples)
│   ├── codenexus_stats.rs — CodeNexus index + stats extraction
│   ├── gitnexus_client.rs — gitnexus MCP cypher client
│   ├── query_compare.rs   — query execution + set comparison
│   ├── report.rs          — Markdown report generation
│   └── type_map.rs        — type mapping loader
└── results/               — generated reports (gitignored)
    ├── *.codenexus.json
    ├── *.gitnexus.json
    ├── *.report.md
    ├── _aggregate.report.md
    └── triage.md
```

## Triage

See [results/triage.md](results/triage.md) for root cause analysis of all critical/major discrepancies across the 8-sample corpus, categorized as:
- **Design differences** — type/edge model choices (no fix needed)
- **Parsing bugs** — genuine extractor issues (need fix change)
- **Test harness issues** — verifier limitations (need fix)
