// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! DDL string generation for the LadybugDB schema (DDD §12.1).
//!
//! Produces the exact DDL strings for the 44 node tables, the `CodeRelation`
//! relationship table, the optional `Embedding` table, and all secondary
//! indexes (DDD §12.2).
//!
//! # CodeRelation design note
//!
//! DDD §5.8 specifies `CREATE REL TABLE CodeRelation (FROM Node TO Node, ...)`,
//! but LadybugDB's `REL TABLE` requires concrete node-table names in the
//! `FROM`/`TO` clauses — there is no generic `Node` union type. To support
//! heterogeneous edges between any of the 44 node tables we materialize
//! `CodeRelation` as a `NODE TABLE` with explicit `source`/`target` string
//! columns (holding node primary keys) plus a synthetic `id` primary key. This
//! preserves every field from the spec while remaining queryable with plain
//! Cypher `MATCH` patterns.
//!
//! # Reserved keyword escaping
//!
//! LadybugDB's Cypher lexer reserves a set of keywords (see `keywords.txt` in
//! the lbug source). Table names that collide with a reserved keyword —
//! notably `Macro` — must be wrapped in backticks when used as identifiers.
//! [`escape_identifier`] handles this transparently for callers.

use crate::model::NodeLabel;

/// LadybugDB reserved keywords that conflict with CodeNexus table names.
///
/// Sourced from `lbug-src/src/antlr4/Cypher.g4` — a keyword is reserved (needs
/// backtick escaping) when it appears in `keywords.txt` but NOT in the
/// `iC_NonReservedKeywords` rule (which lists keywords still usable as
/// identifiers). `PROJECT`/`STRUCT`/`DATABASE` are non-reserved and need no
/// escaping; `MACRO`/`UNION` are reserved and MUST be backtick-escaped.
const RESERVED_KEYWORDS: &[&str] = &["MACRO", "UNION"];

/// Returns `true` if `name` collides with a LadybugDB reserved keyword
/// (case-insensitive comparison).
#[must_use]
pub fn is_reserved_keyword(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    RESERVED_KEYWORDS.iter().any(|kw| *kw == upper)
}

/// Wraps `name` in backticks if it collides with a reserved keyword, otherwise
/// returns it borrowed. Use this whenever a table or column name is spliced
/// into a Cypher statement.
///
/// # Security note (T202-B security-review LOW-1)
///
/// This function ONLY handles reserved-keyword escaping — it does NOT sanitise
/// arbitrary user input. Callers MUST pass values that are known-safe
/// identifiers (hard-coded constants, `NodeLabel::table_name()` results, or
/// values from a fixed enum). Passing user-controlled strings here is a
/// Cypher-injection risk: a malicious `name` containing backticks or single
/// quotes would break out of the identifier context. For user-controlled
/// string values, use [`escape_cypher_string`] inside a single-quoted literal
/// instead.
///
/// # Performance
///
/// Returns [`Cow::Borrowed`] when no escaping is needed (the common case —
/// only `MACRO` and `UNION` are reserved), avoiding a heap allocation per
/// call. The previous `String` return forced `to_string()` on every
/// non-keyword input (perf-review M1: ~12 wasted allocations per
/// `route_map` call on bulwark-class graphs).
#[must_use]
pub fn escape_identifier(name: &str) -> std::borrow::Cow<'_, str> {
    if is_reserved_keyword(name) {
        std::borrow::Cow::Owned(format!("`{name}`"))
    } else {
        std::borrow::Cow::Borrowed(name)
    }
}

/// Escapes a string literal for safe inclusion in a Cypher single-quoted
/// string. Backslash is escaped first, then single quote — the same rule used
/// by every prior local copy (graph_loader, repository, fulltext, structured,
/// disambiguation, rename_cmd) before consolidation here.
///
/// # T202 security-review LOW-1: control character hardening
///
/// The openCypher spec only mandates escaping `\` and `'` inside single-quoted
/// string literals. LadybugDB's Cypher engine follows the spec and treats
/// raw control bytes (`\n`, `\r`, `\t`, etc.) as literal bytes — they do not
/// alter query semantics. However, embedding raw control characters produces
/// malformed log lines (a `\n` breaks the Cypher statement across lines in
/// tracing output) and confuses downstream parsers. We therefore escape the
/// common whitespace control characters to their `\n` / `\r` / `\t` literal
/// forms; the Cypher engine interprets these escape sequences back to the
/// original bytes, so round-trip semantics are preserved while logs and
/// audit trails remain parseable.
#[must_use]
pub fn escape_cypher_string(s: &str) -> String {
    // Order matters: backslash first so we do not double-escape escapes
    // introduced by later steps, then single quote, then control chars.
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// Returns `(table_name, ddl)` pairs for all 44 node tables, in declaration
/// order matching [`NodeLabel::all`].
///
/// The DDL strings are the exact statements from DDD §12.1.
#[must_use]
pub fn node_table_ddl() -> Vec<(&'static str, String)> {
    NodeLabel::all()
        .iter()
        .map(|label| (label.table_name(), ddl_for_label(*label)))
        .collect()
}

/// Returns the DDL for the `CodeRelation` table.
///
/// See the module-level docs for the rationale behind the `NODE TABLE` design.
#[must_use]
pub fn relation_table_ddl() -> String {
    "CREATE NODE TABLE CodeRelation (\
     id STRING, \
     source STRING, \
     target STRING, \
     type STRING, \
     confidence DOUBLE, \
     confidenceTier STRING, \
     reason STRING, \
     startLine INT64, \
     project STRING, \
     PRIMARY KEY (id));"
        .to_string()
}

/// Returns the DDL for the optional `Embedding` table (DDD §5.9).
#[must_use]
pub fn embedding_table_ddl() -> String {
    "CREATE NODE TABLE Embedding (\
     id STRING, \
     nodeId STRING, \
     project STRING, \
     chunkIndex INT32, \
     startLine INT64, \
     endLine INT64, \
     embedding FLOAT[384], \
     contentHash STRING, \
     PRIMARY KEY (id));"
        .to_string()
}

/// Returns all secondary index creation statements (DDD §12.2, §6).
///
/// Includes 18 B-tree secondary indexes, 18 FTS (full-text search) indexes
/// (3 on `content` columns of `Function`/`Class`/`Method` per DDD §6, plus
/// 15 on `name` columns of all symbol tables), and 1
/// VECTOR index on `Embedding.embedding` with cosine distance (DDD §6).
///
/// LadybugDB may not support every index type; [`crate::storage::connection`]
/// skips unsupported statements at init time, recording each skip in the
/// [`SchemaInitReport`].
#[must_use]
pub fn index_ddl() -> Vec<String> {
    vec![
        // --- Secondary indexes (DDD §12.2) ---
        "CREATE INDEX idx_project_name ON Project(name);".to_string(),
        "CREATE INDEX idx_file_project ON File(project);".to_string(),
        "CREATE INDEX idx_file_name ON File(name);".to_string(),
        "CREATE INDEX idx_file_path ON File(filePath);".to_string(),
        "CREATE INDEX idx_file_hash ON File(hash);".to_string(),
        "CREATE INDEX idx_func_project ON Function(project);".to_string(),
        "CREATE INDEX idx_func_name ON Function(name);".to_string(),
        "CREATE INDEX idx_func_qn ON Function(qualifiedName);".to_string(),
        "CREATE INDEX idx_func_path ON Function(filePath);".to_string(),
        "CREATE INDEX idx_class_project ON Class(project);".to_string(),
        "CREATE INDEX idx_class_name ON Class(name);".to_string(),
        "CREATE INDEX idx_class_qn ON Class(qualifiedName);".to_string(),
        "CREATE INDEX idx_var_project ON Variable(project);".to_string(),
        "CREATE INDEX idx_var_name ON Variable(name);".to_string(),
        "CREATE INDEX idx_var_qn ON Variable(qualifiedName);".to_string(),
        "CREATE INDEX idx_var_global ON Variable(isGlobal);".to_string(),
        "CREATE INDEX idx_rel_type ON CodeRelation(type);".to_string(),
        "CREATE INDEX idx_rel_project ON CodeRelation(project);".to_string(),
        // source/target indexes: targeted WHERE e.target = '...' / e.source = '...'
        // queries (api_review::find_handler_for_target, find_callers_of_target,
        // load_edge_reason, trace::graph_loader::fetch_edges_for_node) hit these
        // instead of full-scanning the CodeRelation table. Without them, the
        // bulwark-class graph (94k edges) turns every "targeted" lookup into a
        // 60k+ row linear scan (perf-review C1).
        "CREATE INDEX idx_rel_source ON CodeRelation(source);".to_string(),
        "CREATE INDEX idx_rel_target ON CodeRelation(target);".to_string(),
        // --- FTS indexes (DDD §6): BM25 over symbol `content` columns ---
        "CREATE FTS INDEX fts_function_content ON Function(content);".to_string(),
        "CREATE FTS INDEX fts_class_content ON Class(content);".to_string(),
        "CREATE FTS INDEX fts_method_content ON Method(content);".to_string(),
        // --- FTS indexes: BM25 over symbol `name` columns ---
        // Used by `FullTextSearcher` for identifier-aware BM25 search. The
        // `codenexus_tokenizer` (Rust-side) splits camelCase/snake_case before
        // querying, enabling `parse` to match `parseFile` / `parse_file`.
        // Extended coverage from 3 tables to all 15 symbol
        // tables. `Macro` is backtick-escaped because it is a reserved keyword
        // (see [`is_reserved_keyword`]).
        "CREATE FTS INDEX fts_function_name ON Function(name);".to_string(),
        "CREATE FTS INDEX fts_class_name ON Class(name);".to_string(),
        "CREATE FTS INDEX fts_method_name ON Method(name);".to_string(),
        "CREATE FTS INDEX fts_struct_name ON Struct(name);".to_string(),
        "CREATE FTS INDEX fts_enum_name ON Enum(name);".to_string(),
        "CREATE FTS INDEX fts_trait_name ON Trait(name);".to_string(),
        "CREATE FTS INDEX fts_macro_name ON `Macro`(name);".to_string(),
        "CREATE FTS INDEX fts_typedef_name ON Typedef(name);".to_string(),
        "CREATE FTS INDEX fts_namespace_name ON Namespace(name);".to_string(),
        "CREATE FTS INDEX fts_module_name ON Module(name);".to_string(),
        "CREATE FTS INDEX fts_variable_name ON Variable(name);".to_string(),
        "CREATE FTS INDEX fts_globalvar_name ON GlobalVar(name);".to_string(),
        "CREATE FTS INDEX fts_const_name ON Const(name);".to_string(),
        "CREATE FTS INDEX fts_static_name ON Static(name);".to_string(),
        "CREATE FTS INDEX fts_typealias_name ON TypeAlias(name);".to_string(),
        // --- VECTOR index (DDD §6): cosine similarity over embeddings ---
        "CREATE VECTOR INDEX vec_embedding ON Embedding(embedding) WITH (metric=cosine);"
            .to_string(),
    ]
}

/// Returns the combined DDL statements used to initialize a fresh database.
///
/// Order: node tables (including `Embedding`, sourced from
/// [`embedding_table_ddl`] via [`ddl_for_label`]) → `CodeRelation` → indexes.
///
/// Note: `embedding_table_ddl()` is NOT pushed separately here — it is already
/// included in [`node_table_ddl`] through the `Embedding` variant. Pushing it
/// again would emit a duplicate `CREATE NODE TABLE Embedding` statement and
/// break schema init (Task 2.1 regression).
#[must_use]
pub fn all_init_ddl() -> Vec<String> {
    let mut ddl: Vec<String> = node_table_ddl().into_iter().map(|(_, stmt)| stmt).collect();
    ddl.push(relation_table_ddl());
    ddl.extend(index_ddl());
    ddl
}

/// Returns the column names for a node table, in DDL order.
///
/// Used by the CSV loader to emit header rows and by the repository to build
/// parameterized `CREATE` statements.
#[must_use]
pub fn node_table_columns(label: NodeLabel) -> &'static [&'static str] {
    match label {
        NodeLabel::Project => &[
            "id",
            "name",
            "rootPath",
            "language",
            "fileCount",
            "indexedAt",
            "lastCommit",
        ],
        NodeLabel::Folder => &["id", "project", "name", "filePath"],
        NodeLabel::File => &[
            "id",
            "project",
            "name",
            "filePath",
            "language",
            "hash",
            "lineCount",
        ],
        NodeLabel::Module => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "parentQn",
        ],
        NodeLabel::Class
        | NodeLabel::Struct
        | NodeLabel::Enum
        | NodeLabel::Trait
        | NodeLabel::Interface => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "startLine",
            "endLine",
            "isExported",
            "docstring",
            "content",
            "parentQn",
        ],
        NodeLabel::Impl => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "startLine",
            "endLine",
            "implType",
            "parentQn",
        ],
        NodeLabel::Function => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "startLine",
            "endLine",
            "signature",
            "returnType",
            "isExported",
            "docstring",
            "content",
            "parentQn",
        ],
        NodeLabel::Method => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "startLine",
            "endLine",
            "signature",
            "returnType",
            "isExported",
            "docstring",
            "content",
            "parameterCount",
            "parentQn",
        ],
        NodeLabel::Variable => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "startLine",
            "isGlobal",
            "varType",
            "parentQn",
        ],
        NodeLabel::GlobalVar => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "startLine",
            "varType",
            "isExported",
        ],
        NodeLabel::Parameter => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "startLine",
            "paramType",
            "paramIndex",
            "parentQn",
        ],
        NodeLabel::Const => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "startLine",
            "constType",
            "constValue",
            "isExported",
            "parentQn",
        ],
        NodeLabel::Static => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "startLine",
            "varType",
            "isExported",
            "parentQn",
        ],
        NodeLabel::Macro => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "startLine",
            "endLine",
            "signature",
            "content",
            "parentQn",
        ],
        NodeLabel::TypeAlias => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "startLine",
            "aliasType",
            "isExported",
            "parentQn",
        ],
        NodeLabel::Typedef => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "startLine",
            "typedefType",
            "parentQn",
        ],
        NodeLabel::Namespace => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "parentQn",
        ],
        NodeLabel::Constructor | NodeLabel::Handler | NodeLabel::Middleware | NodeLabel::Test => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "startLine",
            "endLine",
            "signature",
            "returnType",
            "isExported",
            "docstring",
            "content",
            "parentQn",
        ],
        NodeLabel::Record | NodeLabel::Delegate | NodeLabel::Union | NodeLabel::Service => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "startLine",
            "endLine",
            "isExported",
            "docstring",
            "content",
            "parentQn",
        ],
        NodeLabel::Property | NodeLabel::Field => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "startLine",
            "endLine",
            "returnType",
            "isExported",
            "parentQn",
        ],
        NodeLabel::Annotation | NodeLabel::Variant | NodeLabel::Event | NodeLabel::Section => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "startLine",
            "endLine",
            "docstring",
            "parentQn",
        ],
        NodeLabel::Template => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "startLine",
            "endLine",
            "templateParams",
            "parentQn",
        ],
        NodeLabel::Endpoint => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "startLine",
            "endLine",
            "httpMethod",
            "path",
            "expectedSchema",
            "parentQn",
        ],
        NodeLabel::Route => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "startLine",
            "endLine",
            "httpMethod",
            "path",
            "parentQn",
        ],
        NodeLabel::Process | NodeLabel::Community => {
            &["id", "project", "name", "qualifiedName", "docstring"]
        }
        NodeLabel::Database => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "dbType",
            "parentQn",
        ],
        NodeLabel::Config => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "configType",
            "parentQn",
        ],
        NodeLabel::Tool => &[
            "id",
            "project",
            "name",
            "qualifiedName",
            "filePath",
            "toolType",
            "parentQn",
        ],
        // Embedding columns match the vector-store schema (DDD §5.9), not the
        // code-symbol layout. See [`embedding_table_ddl`].
        NodeLabel::Embedding => &[
            "id",
            "nodeId",
            "project",
            "chunkIndex",
            "startLine",
            "endLine",
            "embedding",
            "contentHash",
        ],
    }
}

/// Returns the column names for the `CodeRelation` table, in DDL order.
#[must_use]
pub fn relation_table_columns() -> &'static [&'static str] {
    &[
        "id",
        "source",
        "target",
        "type",
        "confidence",
        "confidenceTier",
        "reason",
        "startLine",
        "project",
    ]
}

/// Builds the exact DDL statement for a single node label.
fn ddl_for_label(label: NodeLabel) -> String {
    match label {
        NodeLabel::Project => {
            "CREATE NODE TABLE Project (id STRING, name STRING, rootPath STRING, \
             language STRING, fileCount INT64, indexedAt INT64, lastCommit STRING, \
             PRIMARY KEY (id));"
                .to_string()
        }
        NodeLabel::Folder => "CREATE NODE TABLE Folder (id STRING, project STRING, name STRING, \
             filePath STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::File => "CREATE NODE TABLE File (id STRING, project STRING, name STRING, \
             filePath STRING, language STRING, hash STRING, lineCount INT64, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Module => "CREATE NODE TABLE Module (id STRING, project STRING, name STRING, \
             qualifiedName STRING, filePath STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Class => "CREATE NODE TABLE Class (id STRING, project STRING, name STRING, \
             qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, isExported \
             BOOLEAN, docstring STRING, content STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Struct => "CREATE NODE TABLE Struct (id STRING, project STRING, name STRING, \
             qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, isExported \
             BOOLEAN, docstring STRING, content STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Enum => "CREATE NODE TABLE Enum (id STRING, project STRING, name STRING, \
             qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, isExported \
             BOOLEAN, docstring STRING, content STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Trait => "CREATE NODE TABLE Trait (id STRING, project STRING, name STRING, \
             qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, isExported \
             BOOLEAN, docstring STRING, content STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Impl => "CREATE NODE TABLE Impl (id STRING, project STRING, name STRING, \
             qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, implType \
             STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Function => "CREATE NODE TABLE Function (id STRING, project STRING, name \
             STRING, qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, \
             signature STRING, returnType STRING, isExported BOOLEAN, docstring STRING, content \
             STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Method => "CREATE NODE TABLE Method (id STRING, project STRING, name STRING, \
             qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, signature \
             STRING, returnType STRING, isExported BOOLEAN, docstring STRING, content STRING, \
             parameterCount INT32, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Variable => "CREATE NODE TABLE Variable (id STRING, project STRING, name \
             STRING, qualifiedName STRING, filePath STRING, startLine INT64, isGlobal BOOLEAN, \
             varType STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::GlobalVar => "CREATE NODE TABLE GlobalVar (id STRING, project STRING, name \
             STRING, qualifiedName STRING, filePath STRING, startLine INT64, varType STRING, \
             isExported BOOLEAN, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Parameter => "CREATE NODE TABLE Parameter (id STRING, project STRING, name \
             STRING, qualifiedName STRING, filePath STRING, startLine INT64, paramType STRING, \
             paramIndex INT32, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Const => "CREATE NODE TABLE Const (id STRING, project STRING, name STRING, \
             qualifiedName STRING, filePath STRING, startLine INT64, constType STRING, \
             constValue STRING, isExported BOOLEAN, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Static => "CREATE NODE TABLE Static (id STRING, project STRING, name STRING, \
             qualifiedName STRING, filePath STRING, startLine INT64, varType STRING, isExported \
             BOOLEAN, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Macro => "CREATE NODE TABLE `Macro` (id STRING, project STRING, name STRING, \
             qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, signature \
             STRING, content STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::TypeAlias => "CREATE NODE TABLE TypeAlias (id STRING, project STRING, name \
             STRING, qualifiedName STRING, filePath STRING, startLine INT64, aliasType STRING, \
             isExported BOOLEAN, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Typedef => "CREATE NODE TABLE Typedef (id STRING, project STRING, name STRING, \
             qualifiedName STRING, filePath STRING, startLine INT64, typedefType STRING, \
             parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Namespace => "CREATE NODE TABLE Namespace (id STRING, project STRING, name \
             STRING, qualifiedName STRING, filePath STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Interface => "CREATE NODE TABLE Interface (id STRING, project STRING, name \
             STRING, qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, \
             isExported BOOLEAN, docstring STRING, content STRING, parentQn STRING, PRIMARY KEY \
             (id));"
            .to_string(),
        NodeLabel::Constructor => "CREATE NODE TABLE Constructor (id STRING, project STRING, \
             name STRING, qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, \
             signature STRING, returnType STRING, isExported BOOLEAN, docstring STRING, content \
             STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Property => "CREATE NODE TABLE Property (id STRING, project STRING, name \
             STRING, qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, \
             returnType STRING, isExported BOOLEAN, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Record => "CREATE NODE TABLE Record (id STRING, project STRING, name STRING, \
             qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, isExported \
             BOOLEAN, docstring STRING, content STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Delegate => "CREATE NODE TABLE Delegate (id STRING, project STRING, name \
             STRING, qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, \
             isExported BOOLEAN, docstring STRING, content STRING, parentQn STRING, PRIMARY KEY \
             (id));"
            .to_string(),
        NodeLabel::Annotation => "CREATE NODE TABLE Annotation (id STRING, project STRING, \
             name STRING, qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, \
             docstring STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Template => "CREATE NODE TABLE Template (id STRING, project STRING, name \
             STRING, qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, \
             templateParams STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Union => "CREATE NODE TABLE `Union` (id STRING, project STRING, name STRING, \
             qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, isExported \
             BOOLEAN, docstring STRING, content STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Variant => "CREATE NODE TABLE Variant (id STRING, project STRING, name \
             STRING, qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, \
             docstring STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Field => "CREATE NODE TABLE Field (id STRING, project STRING, name STRING, \
             qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, returnType \
             STRING, isExported BOOLEAN, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Event => "CREATE NODE TABLE Event (id STRING, project STRING, name STRING, \
             qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, docstring \
             STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Handler => "CREATE NODE TABLE Handler (id STRING, project STRING, name \
             STRING, qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, \
             signature STRING, returnType STRING, isExported BOOLEAN, docstring STRING, content \
             STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Middleware => "CREATE NODE TABLE Middleware (id STRING, project STRING, \
             name STRING, qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, \
             signature STRING, returnType STRING, isExported BOOLEAN, docstring STRING, content \
             STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Service => "CREATE NODE TABLE Service (id STRING, project STRING, name \
             STRING, qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, \
             isExported BOOLEAN, docstring STRING, content STRING, parentQn STRING, PRIMARY KEY \
             (id));"
            .to_string(),
        NodeLabel::Endpoint => "CREATE NODE TABLE Endpoint (id STRING, project STRING, name \
             STRING, qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, \
             httpMethod STRING, path STRING, expectedSchema STRING, parentQn STRING, PRIMARY KEY \
             (id));"
            .to_string(),
        NodeLabel::Route => "CREATE NODE TABLE Route (id STRING, project STRING, name STRING, \
             qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, httpMethod \
             STRING, path STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Process => "CREATE NODE TABLE Process (id STRING, project STRING, name \
             STRING, qualifiedName STRING, docstring STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Database => "CREATE NODE TABLE Database (id STRING, project STRING, name \
             STRING, qualifiedName STRING, filePath STRING, dbType STRING, parentQn STRING, \
             PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Config => "CREATE NODE TABLE Config (id STRING, project STRING, name STRING, \
             qualifiedName STRING, filePath STRING, configType STRING, parentQn STRING, PRIMARY \
             KEY (id));"
            .to_string(),
        NodeLabel::Test => "CREATE NODE TABLE Test (id STRING, project STRING, name STRING, \
             qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, signature \
             STRING, returnType STRING, isExported BOOLEAN, docstring STRING, content STRING, \
             parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Section => "CREATE NODE TABLE Section (id STRING, project STRING, name \
             STRING, qualifiedName STRING, filePath STRING, startLine INT64, endLine INT64, \
             docstring STRING, parentQn STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Community => "CREATE NODE TABLE Community (id STRING, project STRING, name \
             STRING, qualifiedName STRING, docstring STRING, PRIMARY KEY (id));"
            .to_string(),
        NodeLabel::Tool => "CREATE NODE TABLE Tool (id STRING, project STRING, name STRING, \
             qualifiedName STRING, filePath STRING, toolType STRING, parentQn STRING, PRIMARY KEY \
             (id));"
            .to_string(),
        // Embedding is the vector-store table (DDD §5.9 / §12.1), not a code
        // symbol. Its DDL is the canonical source in [`embedding_table_ddl`];
        // delegating here keeps `node_table_ddl()` exhaustive without emitting
        // a duplicate CREATE TABLE in [`all_init_ddl`].
        NodeLabel::Embedding => embedding_table_ddl(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_cypher_string_escapes_backslash_and_quote() {
        // Backslash is escaped first, then single quote.
        assert_eq!(escape_cypher_string("it's a \\test"), "it\\'s a \\\\test");
        assert_eq!(escape_cypher_string("plain"), "plain");
        assert_eq!(escape_cypher_string("'quoted'"), "\\'quoted\\'");
        assert_eq!(escape_cypher_string("back\\slash"), "back\\\\slash");
    }

    #[test]
    fn escape_cypher_string_escapes_control_characters() {
        // T202 security-review LOW-1: \n / \r / \t are escaped to their
        // literal backslash sequences so logs and audit trails remain
        // parseable. The Cypher engine interprets these escape sequences
        // back to the original bytes — round-trip semantics preserved.
        assert_eq!(escape_cypher_string("line1\nline2"), "line1\\nline2");
        assert_eq!(escape_cypher_string("col1\tcol2"), "col1\\tcol2");
        assert_eq!(escape_cypher_string("a\rb"), "a\\rb");
        // Mixed: backslash + quote + control chars.
        assert_eq!(
            escape_cypher_string("it's\na\t\\test\r"),
            "it\\'s\\na\\t\\\\test\\r"
        );
        // Empty string is a no-op.
        assert_eq!(escape_cypher_string(""), "");
    }

    #[test]
    fn node_table_ddl_returns_forty_four_entries() {
        let ddl = node_table_ddl();
        assert_eq!(ddl.len(), 44, "expected 44 node table DDL entries");
    }

    #[test]
    fn node_table_ddl_covers_all_labels() {
        let ddl = node_table_ddl();
        let table_names: Vec<&str> = ddl.iter().map(|(name, _)| *name).collect();
        for label in NodeLabel::all() {
            assert!(
                table_names.contains(&label.table_name()),
                "missing DDL for {}",
                label.table_name()
            );
        }
    }

    #[test]
    fn node_table_ddl_uses_declaration_order() {
        let ddl = node_table_ddl();
        for (i, label) in NodeLabel::all().iter().enumerate() {
            assert_eq!(ddl[i].0, label.table_name(), "mismatch at index {i}");
        }
    }

    #[test]
    fn project_ddl_has_correct_fields() {
        let (_, ddl) = node_table_ddl()
            .into_iter()
            .find(|(name, _)| *name == "Project")
            .expect("Project DDL missing");
        assert!(ddl.contains("CREATE NODE TABLE Project"));
        assert!(ddl.contains("id STRING"));
        assert!(ddl.contains("name STRING"));
        assert!(ddl.contains("rootPath STRING"));
        assert!(ddl.contains("language STRING"));
        assert!(ddl.contains("fileCount INT64"));
        assert!(ddl.contains("indexedAt INT64"));
        assert!(ddl.contains("PRIMARY KEY (id)"));
    }

    #[test]
    fn file_ddl_has_correct_fields() {
        let (_, ddl) = node_table_ddl()
            .into_iter()
            .find(|(name, _)| *name == "File")
            .expect("File DDL missing");
        assert!(ddl.contains("hash STRING"));
        assert!(ddl.contains("lineCount INT64"));
        assert!(ddl.contains("language STRING"));
    }

    #[test]
    fn function_ddl_has_correct_fields() {
        let (_, ddl) = node_table_ddl()
            .into_iter()
            .find(|(name, _)| *name == "Function")
            .expect("Function DDL missing");
        assert!(ddl.contains("signature STRING"));
        assert!(ddl.contains("returnType STRING"));
        assert!(ddl.contains("isExported BOOLEAN"));
        assert!(ddl.contains("docstring STRING"));
        assert!(ddl.contains("content STRING"));
        assert!(ddl.contains("parentQn STRING"));
    }

    #[test]
    fn method_ddl_has_parameter_count() {
        let (_, ddl) = node_table_ddl()
            .into_iter()
            .find(|(name, _)| *name == "Method")
            .expect("Method DDL missing");
        assert!(ddl.contains("parameterCount INT32"));
    }

    #[test]
    fn variable_ddl_has_is_global() {
        let (_, ddl) = node_table_ddl()
            .into_iter()
            .find(|(name, _)| *name == "Variable")
            .expect("Variable DDL missing");
        assert!(ddl.contains("isGlobal BOOLEAN"));
        assert!(ddl.contains("varType STRING"));
    }

    #[test]
    fn parameter_ddl_has_param_index() {
        let (_, ddl) = node_table_ddl()
            .into_iter()
            .find(|(name, _)| *name == "Parameter")
            .expect("Parameter DDL missing");
        assert!(ddl.contains("paramType STRING"));
        assert!(ddl.contains("paramIndex INT32"));
    }

    #[test]
    fn const_ddl_has_const_value() {
        let (_, ddl) = node_table_ddl()
            .into_iter()
            .find(|(name, _)| *name == "Const")
            .expect("Const DDL missing");
        assert!(ddl.contains("constType STRING"));
        assert!(ddl.contains("constValue STRING"));
    }

    #[test]
    fn impl_ddl_has_impl_type() {
        let (_, ddl) = node_table_ddl()
            .into_iter()
            .find(|(name, _)| *name == "Impl")
            .expect("Impl DDL missing");
        assert!(ddl.contains("implType STRING"));
    }

    #[test]
    fn relation_table_ddl_has_required_fields() {
        let ddl = relation_table_ddl();
        assert!(ddl.contains("CodeRelation"));
        assert!(ddl.contains("source STRING"));
        assert!(ddl.contains("target STRING"));
        assert!(ddl.contains("type STRING"));
        assert!(ddl.contains("confidence DOUBLE"));
        assert!(ddl.contains("reason STRING"));
        assert!(ddl.contains("startLine INT64"));
        assert!(ddl.contains("project STRING"));
        assert!(ddl.contains("PRIMARY KEY"));
    }

    #[test]
    fn embedding_table_ddl_has_required_fields() {
        let ddl = embedding_table_ddl();
        assert!(ddl.contains("Embedding"));
        assert!(ddl.contains("id STRING"));
        assert!(ddl.contains("nodeId STRING"));
        assert!(ddl.contains("project STRING"));
        assert!(ddl.contains("chunkIndex INT32"));
        assert!(ddl.contains("startLine INT64"));
        assert!(ddl.contains("endLine INT64"));
        assert!(ddl.contains("embedding FLOAT[384]"));
        assert!(ddl.contains("contentHash STRING"));
    }

    #[test]
    fn index_ddl_contains_all_spec_indexes() {
        let indexes = index_ddl();
        assert!(indexes.iter().any(|s| s.contains("idx_project_name")));
        assert!(indexes.iter().any(|s| s.contains("idx_file_project")));
        assert!(indexes.iter().any(|s| s.contains("idx_file_name")));
        assert!(indexes.iter().any(|s| s.contains("idx_file_path")));
        assert!(indexes.iter().any(|s| s.contains("idx_file_hash")));
        assert!(indexes.iter().any(|s| s.contains("idx_func_project")));
        assert!(indexes.iter().any(|s| s.contains("idx_func_name")));
        assert!(indexes.iter().any(|s| s.contains("idx_func_qn")));
        assert!(indexes.iter().any(|s| s.contains("idx_func_path")));
        assert!(indexes.iter().any(|s| s.contains("idx_class_project")));
        assert!(indexes.iter().any(|s| s.contains("idx_class_name")));
        assert!(indexes.iter().any(|s| s.contains("idx_class_qn")));
        assert!(indexes.iter().any(|s| s.contains("idx_var_project")));
        assert!(indexes.iter().any(|s| s.contains("idx_var_name")));
        assert!(indexes.iter().any(|s| s.contains("idx_var_qn")));
        assert!(indexes.iter().any(|s| s.contains("idx_var_global")));
        assert!(indexes.iter().any(|s| s.contains("idx_rel_type")));
        assert!(indexes.iter().any(|s| s.contains("idx_rel_project")));
        assert!(indexes.iter().any(|s| s.contains("idx_rel_source")));
        assert!(indexes.iter().any(|s| s.contains("idx_rel_target")));
    }

    #[test]
    fn index_ddl_count_matches_spec() {
        let indexes = index_ddl();
        // 20 secondary indexes (18 + idx_rel_source + idx_rel_target)
        // + 18 FTS indexes (3 content + 15 name) + 1 VECTOR index = 39
        assert_eq!(indexes.len(), 39, "expected 39 index statements");
    }

    #[test]
    fn index_ddl_contains_fts_indexes_for_content_columns() {
        // DDD §6: FTS indexes on Function.content, Class.content, Method.content.
        let indexes = index_ddl();
        let fts_indexes: Vec<&String> = indexes
            .iter()
            .filter(|s| s.to_ascii_uppercase().contains("FTS"))
            .collect();
        assert_eq!(
            fts_indexes.len(),
            18,
            "expected exactly 18 FTS index statements (3 content + 15 name), got {}: {fts_indexes:?}",
            fts_indexes.len()
        );
        // Each FTS statement must target the `content` column of its table.
        assert!(
            indexes.iter().any(|s| {
                let up = s.to_ascii_uppercase();
                up.contains("FTS") && up.contains("FUNCTION") && up.contains("CONTENT")
            }),
            "missing FTS index on Function(content): {indexes:?}"
        );
        assert!(
            indexes.iter().any(|s| {
                let up = s.to_ascii_uppercase();
                up.contains("FTS") && up.contains("CLASS") && up.contains("CONTENT")
            }),
            "missing FTS index on Class(content): {indexes:?}"
        );
        assert!(
            indexes.iter().any(|s| {
                let up = s.to_ascii_uppercase();
                up.contains("FTS") && up.contains("METHOD") && up.contains("CONTENT")
            }),
            "missing FTS index on Method(content): {indexes:?}"
        );
    }

    #[test]
    fn index_ddl_contains_fts_indexes_for_name_columns() {
        // FTS indexes on the `name` column of all 15
        // symbol-bearing tables for identifier-aware BM25 search via
        // codenexus_tokenizer.
        let indexes = index_ddl();
        let fts_name_indexes: Vec<&String> = indexes
            .iter()
            .filter(|s| {
                let up = s.to_ascii_uppercase();
                up.contains("FTS") && up.contains("NAME")
            })
            .collect();
        assert_eq!(
            fts_name_indexes.len(),
            15,
            "expected 15 FTS name indexes, got {}: {fts_name_indexes:?}",
            fts_name_indexes.len()
        );
        // Spot-check the original 3 + a few new ones.
        for table in [
            "FUNCTION",
            "CLASS",
            "METHOD",
            "STRUCT",
            "ENUM",
            "TRAIT",
            "MACRO",
            "TYPEDEF",
            "NAMESPACE",
            "MODULE",
            "VARIABLE",
            "GLOBALVAR",
            "CONST",
            "STATIC",
            "TYPEALIAS",
        ] {
            assert!(
                indexes.iter().any(|s| {
                    let up = s.to_ascii_uppercase();
                    up.contains("FTS") && up.contains(table) && up.contains("NAME")
                }),
                "missing FTS index on {table}(name): {indexes:?}"
            );
        }
    }

    #[test]
    fn index_ddl_contains_vector_index_for_embedding() {
        // DDD §6: VECTOR index on Embedding.embedding with cosine distance.
        let indexes = index_ddl();
        let vec_indexes: Vec<&String> = indexes
            .iter()
            .filter(|s| s.to_ascii_uppercase().contains("VECTOR"))
            .collect();
        assert_eq!(
            vec_indexes.len(),
            1,
            "expected exactly 1 VECTOR index statement, got {}: {vec_indexes:?}",
            vec_indexes.len()
        );
        let stmt = vec_indexes[0];
        let up = stmt.to_ascii_uppercase();
        assert!(
            up.contains("EMBEDDING"),
            "VECTOR index must target the Embedding table: {stmt}"
        );
        assert!(
            up.contains("COSINE"),
            "VECTOR index must use cosine metric: {stmt}"
        );
    }

    #[test]
    fn all_init_ddl_includes_node_tables_relation_embedding_and_indexes() {
        let ddl = all_init_ddl();
        // 44 node tables (incl. Embedding via ddl_for_label) + 1 relation
        // + 39 indexes (20 secondary + 18 FTS + 1 VECTOR) = 84
        // (v0.3.7: added idx_rel_source + idx_rel_target for targeted WHERE
        // e.target/e.source lookups — perf-review C1)
        assert_eq!(ddl.len(), 84, "expected 84 DDL statements total");
        assert!(ddl.iter().any(|s| s.contains("CREATE NODE TABLE Project")));
        assert!(ddl.iter().any(|s| s.contains("CodeRelation")));
        assert!(ddl.iter().any(|s| s.contains("Embedding")));
        assert!(ddl.iter().any(|s| s.contains("CREATE INDEX")));
        assert!(ddl.iter().any(|s| s.to_ascii_uppercase().contains("FTS")));
        assert!(ddl
            .iter()
            .any(|s| s.to_ascii_uppercase().contains("VECTOR")));
    }

    #[test]
    fn node_table_columns_match_ddl_for_each_label() {
        for label in NodeLabel::all() {
            let cols = node_table_columns(label);
            assert!(!cols.is_empty(), "no columns for {label}");
            assert!(cols.contains(&"id"), "id missing for {label}");
        }
    }

    #[test]
    fn node_table_columns_for_function() {
        let cols = node_table_columns(NodeLabel::Function);
        assert_eq!(
            cols,
            &[
                "id",
                "project",
                "name",
                "qualifiedName",
                "filePath",
                "startLine",
                "endLine",
                "signature",
                "returnType",
                "isExported",
                "docstring",
                "content",
                "parentQn",
            ]
        );
    }

    #[test]
    fn node_table_columns_for_project() {
        let cols = node_table_columns(NodeLabel::Project);
        assert_eq!(
            cols,
            &[
                "id",
                "name",
                "rootPath",
                "language",
                "fileCount",
                "indexedAt",
                "lastCommit"
            ]
        );
    }

    #[test]
    fn relation_table_columns_has_nine_columns() {
        let cols = relation_table_columns();
        assert_eq!(
            cols,
            &[
                "id",
                "source",
                "target",
                "type",
                "confidence",
                "confidenceTier",
                "reason",
                "startLine",
                "project",
            ]
        );
    }

    #[test]
    fn class_struct_enum_trait_share_column_layout() {
        let class = node_table_columns(NodeLabel::Class);
        let struct_ = node_table_columns(NodeLabel::Struct);
        let enum_ = node_table_columns(NodeLabel::Enum);
        let trait_ = node_table_columns(NodeLabel::Trait);
        assert_eq!(class, struct_);
        assert_eq!(class, enum_);
        assert_eq!(class, trait_);
    }

    #[test]
    fn is_reserved_keyword_detects_macro() {
        assert!(is_reserved_keyword("Macro"));
        assert!(is_reserved_keyword("MACRO"));
        assert!(is_reserved_keyword("macro"));
    }

    #[test]
    fn is_reserved_keyword_returns_false_for_non_keywords() {
        assert!(!is_reserved_keyword("Project"));
        assert!(!is_reserved_keyword("Function"));
        assert!(!is_reserved_keyword("CodeRelation"));
        assert!(!is_reserved_keyword(""));
    }

    #[test]
    fn escape_identifier_wraps_macro_in_backticks() {
        assert_eq!(escape_identifier("Macro"), "`Macro`");
        assert_eq!(escape_identifier("MACRO"), "`MACRO`");
    }

    #[test]
    fn escape_identifier_leaves_non_keywords_unchanged() {
        assert_eq!(escape_identifier("Project"), "Project");
        assert_eq!(escape_identifier("Function"), "Function");
        assert_eq!(escape_identifier("CodeRelation"), "CodeRelation");
    }

    #[test]
    fn macro_ddl_uses_backtick_escaping() {
        let (_, ddl) = node_table_ddl()
            .into_iter()
            .find(|(name, _)| *name == "Macro")
            .expect("Macro DDL missing");
        assert!(
            ddl.contains("CREATE NODE TABLE `Macro`"),
            "Macro DDL must escape the reserved keyword: {ddl}"
        );
        assert!(ddl.contains("signature STRING"));
        assert!(ddl.contains("content STRING"));
    }
}
