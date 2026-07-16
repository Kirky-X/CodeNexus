# Storage Model Reference

> CodeNexus graph schema: CodeRelation node table, 44 node types, and 24 edge types. Part of the CodeNexus skill — see [SKILL.md](../SKILL.md) for the overview.

## CodeRelation Node Table

> Important for `query` users: CodeNexus stores edges as **nodes** in a `CodeRelation` NODE TABLE, not as LadybugDB REL relationships. This is by design (see `src/storage/schema.rs:80-97`): LadybugDB's `REL TABLE` requires concrete node-table names for FROM/TO and cannot express a general edge across 44 node types.

**Implication for Cypher:**
- `MATCH ()-[r]->()` and `MATCH ()-[r:CALLS]->()` **return 0** — there are no REL relationships.
- To traverse edges, query the `CodeRelation` node table:
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
- `CodeRelation` columns: `id`, `source`, `target`, `type`, `confidence`, `confidenceTier`, `reason`, `startLine`, `project`. `source`/`target` hold the symbol qualifiedName (equal to `Function.id`).
- High-level commands (`trace`, `impact`, `dead_code`, `architecture`, `community`, etc.) abstract over this layout — they read `CodeRelation` internally so you don't have to write the join yourself.

## Node Types (44)

**Structural (4):** Project, Folder, File, Module
**Type definitions (5):** Class, Struct, Enum, Trait, Impl
**Callables (2):** Function, Method
**Variables (5):** Variable, GlobalVar, Parameter, Const, Static
**Meta (5):** Macro, TypeAlias, Typedef, Namespace, Interface
**H1 Type definitions (5):** Constructor, Property, Record, Delegate, Annotation
**H1 Templates (1):** Template
**H1 Union/Variant/Field (3):** Union, Variant, Field
**H1 Runtime/architecture (7):** Event, Handler, Middleware, Service, Endpoint, Route, Process
**H1 Data/infra (2):** Database, Config
**H1 Quality/docs (2):** Test, Section
**H1 Community/extension (3):** Community, Tool, Embedding

## Edge Types

**Original (14):** CONTAINS, DEFINES, MEMBER_OF, CALLS, FFI_CALLS, DATAFLOWS, READS, WRITES, IMPLEMENTS, EXTENDS, USES_TYPE, REFERENCES, IMPORTS, INCLUDES
**H1 T9 extension (10):** HAS_METHOD, HAS_PROPERTY, ACCESSES, METHOD_OVERRIDES, METHOD_IMPLEMENTS, STEP_IN_PROCESS, HANDLES_ROUTE, FETCHES, HANDLES_TOOL, ENTRY_POINT_OF

Each `CodeRelation` row carries a `confidence` score in `[0.0, 1.0]` and a `confidenceTier` (`SameFile` / `ImportScoped` / `Global`) populated during resolution. Use `--edge_types` on `impact` and `--path_filter` on `trace` to scope results by edge type or file path (design.md D4).
