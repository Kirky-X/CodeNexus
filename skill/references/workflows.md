# Typical Workflows

> Nine end-to-end workflows using the verified 0.3.4 flag syntax. Part of the CodeNexus skill — see [SKILL.md](../SKILL.md) for the overview.

## Typical Workflows

> Every command below uses the **verified 0.3.4 flag syntax** (no positional args, mandatory flags supplied, booleans value-styled). `--project` accepts either name or id.

### Workflow 1: Index and explore a new codebase

```bash
codenexus index --path /path/to/repo --name myproject --force false --lsp false --embed false --ram_first false
codenexus query --cypher "MATCH (f:Function) RETURN f.name, f.filePath, f.startLine ORDER BY f.name LIMIT 50"
# search is currently unreliable (see caveat); use query for symbol lookup:
codenexus query --cypher "MATCH (f:Function) WHERE f.name CONTAINS 'parse' RETURN f.name, f.filePath LIMIT 20"
codenexus trace --symbol main --trace_type calls --depth 5 --path_filter "" --detect_cycles false --cross_service false
codenexus impact --symbol critical_function --depth 3 --edge_types "CALLS" --max_depth 3 --include_tests false
```

### Workflow 2: Continuous indexing with daemon

```bash
codenexus index --path /path/to/repo --name myproject --force false --lsp false --embed false --ram_first false
codenexus daemon --path /path/to/repo --name myproject --debounce-ms 1000
# In another terminal, query the live graph:
codenexus query --cypher "MATCH (r:CodeRelation) WHERE r.type='CALLS' RETURN r.source, r.target LIMIT 20"
```

### Workflow 3: Multi-project management

```bash
codenexus index --path /path/to/project-a --name projectA --force false --lsp false --embed false --ram_first false --db /shared/graph.lbug
codenexus index --path /path/to/project-b --name projectB --force false --lsp false --embed false --ram_first false --db /shared/graph.lbug
codenexus list --db /shared/graph.lbug
# clean accepts either name or id:
codenexus clean --project projectA --db /shared/graph.lbug
```

### Workflow 4: Cross-language FFI tracing

```bash
codenexus index --path /path/to/mixed-repo --name ffiproject --force false --lsp false --embed false --ram_first false
codenexus trace --symbol rust_entry_point --trace_type calls --depth 10 --path_filter "" --detect_cycles false --cross_service false
codenexus query --cypher "MATCH (a:Function)-[:FFI_CALLS]->(b:Function) RETURN a.name, b.name, a.filePath, b.filePath"
```

### Workflow 5: Refactoring with multi-dimensional impact

```bash
# Narrow edge types + max_depth to keep impact tractable on large graphs (see performance note)
codenexus impact --symbol critical_function --depth 3 --edge_types "CALLS,IMPLEMENTS,USES_TYPE" --max_depth 3 --include_tests false
codenexus trace --symbol data_var --trace_type dataflow --depth 3 --path_filter "/src/**" --detect_cycles false --cross_service false
# Detect what a git change touches, then propose a rename
codenexus detect_changes --path /repo --mode unstaged
codenexus rename --from old_name --to new_name --path /repo --apply false --db /repo/codenexus.lbug
codenexus rename --from old_name --to new_name --path /repo --apply true
```

### Workflow 6: Team artifact sharing

```bash
# On machine A: export the indexed graph
codenexus export --output myproject.graph.zst --project myproject --db /work/graph.lbug
# On machine B: import and reindex local diff
codenexus import --input myproject.graph.zst --reindex true --path /repo --name myproject --db /work/graph.lbug
```

### Workflow 7: MCP integration with AI agents

```bash
# One-time: write MCP config into detected agents
codenexus setup --force false
# Agents then launch `codenexus mcp` automatically; or run manually for testing:
codenexus mcp --db /work/graph.lbug
```

### Workflow 8: Complexity audit

```bash
# All 26 threshold flags are required. Pass 0/"" to keep in-memory defaults.
codenexus complexity \
  --project myproject \
  --red_only false --sort_by_severity true \
  --cyclomatic_green 0 --cyclomatic_yellow 0 --cyclomatic_red 0 \
  --cognitive_green 0 --cognitive_yellow 0 --cognitive_red 0 \
  --nesting_green 0 --nesting_yellow 0 --nesting_red 0 \
  --func_length_green 0 --func_length_yellow 0 --func_length_red 0 \
  --halstead_volume_green 0 --halstead_volume_yellow 0 --halstead_volume_red 0 \
  --maintainability_green 0 --maintainability_yellow 0 --maintainability_red 0 \
  --time_complexity_green "" --time_complexity_yellow "" --time_complexity_red "" \
  --space_complexity_yellow "" --space_complexity_red ""
```

### Workflow 9: API surface analysis

```bash
codenexus route_map --project myproject          # list HTTP routes + handlers
codenexus tool_map --project myproject           # list MCP tools + handlers
codenexus shape_check --project myproject        # check endpoint shape consistency
codenexus api_impact --project myproject --endpoint "/api/v1/users"
codenexus cross_service --project myproject --protocol ""
```
