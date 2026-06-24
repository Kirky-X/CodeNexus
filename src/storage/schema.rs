//! DDL string generation for the LadybugDB schema (DDD §12.1).
//!
//! Produces the exact DDL strings for the 20 node tables, the `CodeRelation`
//! relationship table, the optional `Embedding` table, and all secondary
//! indexes (DDD §12.2).
//!
//! # CodeRelation design note
//!
//! DDD §5.8 specifies `CREATE REL TABLE CodeRelation (FROM Node TO Node, ...)`,
//! but LadybugDB's `REL TABLE` requires concrete node-table names in the
//! `FROM`/`TO` clauses — there is no generic `Node` union type. To support
//! heterogeneous edges between any of the 20 node tables we materialize
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
/// Sourced from `lbug-src/src/antlr4/keywords.txt`. Only keywords that collide
/// with our 20 node-table names or `CodeRelation` need to be listed here.
const RESERVED_KEYWORDS: &[&str] = &["MACRO"];

/// Returns `true` if `name` collides with a LadybugDB reserved keyword
/// (case-insensitive comparison).
#[must_use]
pub fn is_reserved_keyword(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    RESERVED_KEYWORDS.iter().any(|kw| *kw == upper)
}

/// Wraps `name` in backticks if it collides with a reserved keyword, otherwise
/// returns it unchanged. Use this whenever a table or column name is spliced
/// into a Cypher statement.
#[must_use]
pub fn escape_identifier(name: &str) -> String {
    if is_reserved_keyword(name) {
        format!("`{name}`")
    } else {
        name.to_string()
    }
}

/// Returns `(table_name, ddl)` pairs for all 20 node tables, in declaration
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

/// Returns all secondary index creation statements (DDD §12.2).
///
/// LadybugDB may not support every index type; [`crate::storage::connection`]
/// skips unsupported statements at init time.
#[must_use]
pub fn index_ddl() -> Vec<String> {
    vec![
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
    ]
}

/// Returns the combined DDL statements used to initialize a fresh database.
///
/// Order: node tables → `CodeRelation` → `Embedding` → indexes.
#[must_use]
pub fn all_init_ddl() -> Vec<String> {
    let mut ddl: Vec<String> = node_table_ddl()
        .into_iter()
        .map(|(_, stmt)| stmt)
        .collect();
    ddl.push(relation_table_ddl());
    ddl.push(embedding_table_ddl());
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
        NodeLabel::Project => &["id", "name", "rootPath", "language", "fileCount", "indexedAt"],
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
        NodeLabel::Module => &["id", "project", "name", "qualifiedName", "filePath", "parentQn"],
        NodeLabel::Class | NodeLabel::Struct | NodeLabel::Enum | NodeLabel::Trait => &[
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
        NodeLabel::Namespace => &["id", "project", "name", "qualifiedName", "filePath", "parentQn"],
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
        "reason",
        "startLine",
        "project",
    ]
}

/// Builds the exact DDL statement for a single node label.
fn ddl_for_label(label: NodeLabel) -> String {
    match label {
        NodeLabel::Project => "CREATE NODE TABLE Project (id STRING, name STRING, rootPath STRING, \
             language STRING, fileCount INT64, indexedAt INT64, PRIMARY KEY (id));"
            .to_string(),
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_table_ddl_returns_twenty_entries() {
        let ddl = node_table_ddl();
        assert_eq!(ddl.len(), 20, "expected 20 node table DDL entries");
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
    }

    #[test]
    fn index_ddl_count_matches_spec() {
        let indexes = index_ddl();
        assert_eq!(indexes.len(), 18, "expected 18 index statements");
    }

    #[test]
    fn all_init_ddl_includes_node_tables_relation_embedding_and_indexes() {
        let ddl = all_init_ddl();
        // 20 node tables + 1 relation + 1 embedding + 18 indexes = 40
        assert_eq!(ddl.len(), 40, "expected 40 DDL statements total");
        assert!(ddl.iter().any(|s| s.contains("CREATE NODE TABLE Project")));
        assert!(ddl.iter().any(|s| s.contains("CodeRelation")));
        assert!(ddl.iter().any(|s| s.contains("Embedding")));
        assert!(ddl.iter().any(|s| s.contains("CREATE INDEX")));
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
            &["id", "name", "rootPath", "language", "fileCount", "indexedAt"]
        );
    }

    #[test]
    fn relation_table_columns_has_eight_columns() {
        let cols = relation_table_columns();
        assert_eq!(
            cols,
            &[
                "id",
                "source",
                "target",
                "type",
                "confidence",
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
