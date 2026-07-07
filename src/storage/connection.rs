// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Connection wrapper around [`lbug::Database`] / [`lbug::Connection`].
//!
//! [`lbug::Connection`] borrows [`lbug::Database`] for its entire lifetime, so
//! we cannot store both in a single struct without self-referential plumbing.
//! Following the design note in the task spec we instead **own** the
//! [`Database`] and create short-lived [`Connection`]s on demand for each
//! `execute` / `query` call. This trades a small per-call overhead for
//! simplicity and testability.

use std::path::Path;

use lbug::{Connection, Database, SystemConfig, Value};
use tracing::warn;

use super::error::{Result, StorageError};
use super::schema::all_init_ddl;

/// Report of schema initialization outcome.
///
/// Returned by [`StorageConnection::init_schema`] to make skipped DDL
/// statements visible to callers instead of silently swallowing them.
/// A non-empty `skipped_reasons` indicates DDL the linked LadybugDB build
/// could not execute (e.g. unsupported index syntax, or a table that
/// already exists on re-init).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchemaInitReport {
    /// Number of DDL statements skipped (unsupported or already exists).
    pub skipped_count: u32,
    /// Reasons for skipping, one per skipped statement.
    pub skipped_reasons: Vec<String>,
}

/// Returns `true` if the error message indicates database corruption.
///
/// Detects LadybugDB / SQLite corruption patterns (spec
/// `test-coverage-completion-spec`):
/// - `database disk image is malformed` (SQLite)
/// - `file is encrypted or is not a database` (SQLite)
/// - `no such table: node_function` (required table missing)
/// - `database schema has changed` (SQLite)
/// - `not a valid` + `database` (LadybugDB: "The file is not a valid Lbug
///   database file!" — observed at runtime; the spec's SQLite-style patterns
///   do not match LadybugDB's actual message, so this compound check is
///   added to cover the real error)
///
/// Explicitly **excludes** `database is locked` — locking is a transient
/// condition handled by retry logic, not corruption.
fn is_corruption_error(e: &StorageError) -> bool {
    let msg = e.to_string().to_lowercase();
    msg.contains("database disk image is malformed")
        || msg.contains("file is encrypted or is not a database")
        || msg.contains("no such table: node_function")
        || msg.contains("database schema has changed")
        || (msg.contains("not a valid") && msg.contains("database"))
}

/// A wrapper around a LadybugDB [`Database`] providing schema initialization,
/// DDL/DML execution, and JSON-valued query helpers.
///
/// Cloning is intentionally **not** supported — each instance owns its
/// underlying database files. To share data across logical owners, open the
/// same path from multiple [`StorageConnection`] instances (LadybugDB supports
/// multi-process access).
pub struct StorageConnection {
    db: Database,
}

impl StorageConnection {
    /// Opens (or creates) a LadybugDB database at `path`.
    ///
    /// If `path` does not exist it will be created. Pass `":memory:"` to get
    /// an in-memory database (useful for tests).
    ///
    /// When `Database::new` fails with a corruption-pattern error (e.g.
    /// "database disk image is malformed"), the error is wrapped as
    /// [`StorageError::Corrupt`] so the upper layer's `From<StorageError>`
    /// impl maps it to [`IndexError::DatabaseCorrupt`] (exit code 4).
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        // Test builds run many StorageConnection instances in parallel under
        // `cargo test`. LadybugDB's default `buffer_pool_size = 0` lets DuckDB
        // auto-detect the buffer pool, which resolves to ~8 TiB and triggers
        // `Mmap for size 8796093022208 failed` once N parallel instances each
        // try to reserve 8 TiB of virtual address space (Rule 12: fail loud —
        // the original failure surfaced as `.expect("in_memory ...")` panics
        // scattered across ~120 tests). Pin a 256 MiB cap in test builds so
        // parallel in-memory DBs stay within reasonable per-process limits.
        // Production builds keep the auto-detect behavior (buffer_pool_size = 0).
        let config = if cfg!(test) {
            // Test builds run many StorageConnection instances in parallel
            // under `cargo test`. Two SystemConfig defaults trigger 8 TiB
            // mmap requests per instance, exhausting the kernel's virtual
            // address budget once N instances run together:
            //   1. buffer_pool_size = 0 → C++ auto-detects to ~80% of phys
            //      mem, capped at DEFAULT_VM_REGION_MAX_SIZE (8 TiB on 64-bit
            //      Linux — see lbug constants.h L63).
            //   2. max_db_size = u32::MAX (0xFFFFFFFF) → C++ treats this as
            //      "unset" and replaces it with DEFAULT_VM_REGION_MAX_SIZE
            //      (database.cpp L82-84), then mmaps that as the BM region.
            // The resulting `Mmap for size 8796093022208 failed` panic
            // surfaces in ~120 tests via `.expect("in_memory ...")`. Pin both
            // to small explicit values so parallel in-memory DBs stay within
            // reasonable per-process limits (Rule 12: fail loud, not silent).
            // Production builds keep the auto-detect behavior.
            SystemConfig::default()
                .buffer_pool_size(256 * 1024 * 1024)
                .max_db_size(1024 * 1024 * 1024)
        } else {
            SystemConfig::default()
        };
        match Database::new(path, config) {
            Ok(db) => Ok(Self { db }),
            Err(e) => {
                let storage_err = StorageError::Database(e);
                if is_corruption_error(&storage_err) {
                    Err(StorageError::Corrupt(storage_err.to_string()))
                } else {
                    Err(storage_err)
                }
            }
        }
    }

    /// Creates an in-memory database (alias for `open(":memory:")`).
    pub fn in_memory() -> Result<Self> {
        Self::open(":memory:")
    }

    /// Initializes the full CodeNexus schema: all 20 node tables, the
    /// `CodeRelation` table, the optional `Embedding` table, and all secondary
    /// indexes (DDD §12.1, §12.2).
    ///
    /// Statements that LadybugDB cannot execute (e.g. unsupported index syntax
    /// or a table that already exists on re-init) are logged as warnings,
    /// recorded in the returned [`SchemaInitReport`], and skipped — they do
    /// not abort initialization. Genuine failures (e.g. an invalid column
    /// type) do abort initialization by returning [`StorageError::Schema`].
    pub fn init_schema(&self) -> Result<SchemaInitReport> {
        let ddl = all_init_ddl();
        self.run_init_ddl(&ddl)
    }

    /// Executes a list of DDL statements, classifying each failure as either
    /// "unsupported" (skipped and recorded) or a real error (returned).
    ///
    /// A failure is treated as "unsupported" when its error message indicates
    /// the database cannot handle the statement:
    /// - `not supported` / `already exists` / `does not exist` — semantic
    ///   signals from the binder/catalog.
    /// - `Parser exception` — the parser does not recognize the DDL syntax
    ///   (e.g. `CREATE INDEX ... ON` is unsupported by this LadybugDB build).
    ///   In `init_schema` every statement originates from [`all_init_ddl`],
    ///   so a parse failure means "unsupported feature", not "invalid SQL".
    ///
    /// Any other failure (e.g. `Catalog exception` for an unknown type) is a
    /// real error and is propagated as [`StorageError::Schema`].
    fn run_init_ddl(&self, ddl: &[String]) -> Result<SchemaInitReport> {
        let mut report = SchemaInitReport::default();
        for stmt in ddl {
            if let Err(err) = self.execute(stmt) {
                let msg = err.to_string();
                let is_unsupported = msg.contains("not supported")
                    || msg.contains("already exists")
                    || msg.contains("does not exist")
                    || msg.contains("Parser exception");
                if is_unsupported {
                    warn!(statement = %stmt, error = %msg, "skipping unsupported DDL statement");
                    report.skipped_count += 1;
                    report.skipped_reasons.push(format!("`{stmt}`: {msg}"));
                } else {
                    let schema_err = StorageError::Schema(format!(
                        "failed to execute DDL `{stmt}`: {msg}"
                    ));
                    if is_corruption_error(&schema_err) {
                        return Err(StorageError::Corrupt(schema_err.to_string()));
                    }
                    return Err(schema_err);
                }
            }
        }
        Ok(report)
    }

    /// Executes a single Cypher statement that does not return rows (DDL/DML).
    pub fn execute(&self, cypher: &str) -> Result<()> {
        let conn = Connection::new(&self.db)?;
        let mut result = conn.query(cypher)?;
        // Drain to surface any execution errors that arrive lazily.
        while result.next().is_some() {}
        Ok(())
    }

    /// Executes a Cypher query and returns all rows as a vector of JSON value
    /// vectors.
    ///
    /// Each inner `Vec<serde_json::Value>` corresponds to one row; each element
    /// is one column converted from [`lbug::Value`] via [`value_to_json`].
    pub fn query(&self, cypher: &str) -> Result<Vec<Vec<serde_json::Value>>> {
        let conn = Connection::new(&self.db)?;
        let mut result = conn.query(cypher)?;
        let mut rows = Vec::with_capacity(result.get_num_tuples() as usize);
        for row in &mut result {
            let json_row = row.into_iter().map(value_to_json).collect();
            rows.push(json_row);
        }
        Ok(rows)
    }

    /// Executes a Cypher query and returns the column names alongside the rows.
    ///
    /// Useful for callers that need to map values back to named fields.
    pub fn query_with_columns(
        &self,
        cypher: &str,
    ) -> Result<(Vec<String>, Vec<Vec<serde_json::Value>>)> {
        let conn = Connection::new(&self.db)?;
        let mut result = conn.query(cypher)?;
        let columns = result.get_column_names();
        let mut rows = Vec::with_capacity(result.get_num_tuples() as usize);
        for row in &mut result {
            let json_row = row.into_iter().map(value_to_json).collect();
            rows.push(json_row);
        }
        Ok((columns, rows))
    }

    /// Returns a borrowed [`Connection`] for callers that need direct access to
    /// the underlying LadybugDB API (e.g. prepared statements).
    ///
    /// The connection borrows `self` for the duration of the closure.
    pub fn with_connection<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection<'_>) -> Result<T>,
    {
        let conn = Connection::new(&self.db)?;
        f(&conn)
    }
}

impl std::fmt::Debug for StorageConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageConnection")
            .field("db", &"Opaque lbug::Database")
            .finish()
    }
}

/// Converts an [`lbug::Value`] into a [`serde_json::Value`].
///
/// Numeric types are normalized to `i64` / `f64` where possible; unsupported
/// variants (e.g. `Node`, `Rel`, `RecursiveRel`) fall back to their string
/// representation.
#[must_use]
pub fn value_to_json(value: Value) -> serde_json::Value {
    match value {
        Value::Null(_) => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(b),
        Value::Int8(i) => serde_json::json!(i64::from(i)),
        Value::Int16(i) => serde_json::json!(i64::from(i)),
        Value::Int32(i) => serde_json::json!(i64::from(i)),
        Value::Int64(i) => serde_json::json!(i),
        Value::UInt8(u) => serde_json::json!(i64::from(u)),
        Value::UInt16(u) => serde_json::json!(i64::from(u)),
        Value::UInt32(u) => serde_json::json!(i64::from(u)),
        Value::UInt64(u) => serde_json::json!(u.to_string()),
        Value::Int128(i) => serde_json::json!(i.to_string()),
        Value::Float(fl) => serde_json::json!(f64::from(fl)),
        Value::Double(d) => serde_json::json!(d),
        Value::String(s) => serde_json::Value::String(s),
        Value::Json(v) => v,
        Value::Blob(b) => serde_json::Value::String(
            String::from_utf8_lossy(&b).into_owned(),
        ),
        Value::Date(d) => serde_json::Value::String(d.to_string()),
        Value::Timestamp(t)
        | Value::TimestampTz(t)
        | Value::TimestampNs(t)
        | Value::TimestampMs(t)
        | Value::TimestampSec(t) => serde_json::Value::String(t.to_string()),
        Value::Interval(d) => serde_json::Value::String(d.to_string()),
        Value::UUID(u) => serde_json::Value::String(u.to_string()),
        Value::Decimal(d) => serde_json::Value::String(d.to_string()),
        Value::InternalID(id) => serde_json::json!(format!("{}:{}", id.table_id, id.offset)),
        Value::List(_, items) | Value::Array(_, items) => {
            serde_json::Value::Array(items.into_iter().map(value_to_json).collect())
        }
        Value::Struct(fields) => {
            let mut map = serde_json::Map::new();
            for (name, val) in fields {
                map.insert(name, value_to_json(val));
            }
            serde_json::Value::Object(map)
        }
        Value::Map(_, entries) => {
            let mut map = serde_json::Map::new();
            for (key, val) in entries {
                let key_str = match key {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                map.insert(key_str, value_to_json(val));
            }
            serde_json::Value::Object(map)
        }
        Value::Node(node) => serde_json::Value::String(node.to_string()),
        Value::Rel(rel) => serde_json::Value::String(rel.to_string()),
        Value::RecursiveRel { nodes, rels } => {
            serde_json::json!({
                "nodes": nodes.iter().map(|n| n.to_string()).collect::<Vec<_>>(),
                "rels": rels.iter().map(|r| r.to_string()).collect::<Vec<_>>(),
            })
        }
        Value::Union { value, .. } => value_to_json(*value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_conn() -> StorageConnection {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = StorageConnection::open(dir.path().join("testdb")).expect("open");
        // Leak the tempdir so the database files survive for the test's lifetime.
        // LadybugDB keeps file handles open; dropping the TempDir would delete
        // them out from under us. This is acceptable for short-lived tests.
        std::mem::forget(dir);
        conn
    }

    #[test]
    fn open_creates_database() {
        let dir = tempfile::tempdir().unwrap();
        let conn = StorageConnection::open(dir.path().join("testdb"));
        assert!(conn.is_ok(), "failed to open database: {:?}", conn.err());
        dir.close().unwrap();
    }

    #[test]
    fn in_memory_works() {
        let conn = StorageConnection::in_memory();
        assert!(conn.is_ok());
    }

    #[test]
    fn init_schema_creates_all_tables() {
        let conn = fresh_conn();
        conn.init_schema().expect("init_schema failed");

        // Verify a representative set of tables exist by querying them.
        let rows = conn
            .query("MATCH (p:Project) RETURN count(p) AS cnt;")
            .expect("query Project failed");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], serde_json::json!(0));

        let rows = conn
            .query("MATCH (f:Function) RETURN count(f) AS cnt;")
            .expect("query Function failed");
        assert_eq!(rows[0][0], serde_json::json!(0));

        let rows = conn
            .query("MATCH (c:Class) RETURN count(c) AS cnt;")
            .expect("query Class failed");
        assert_eq!(rows[0][0], serde_json::json!(0));
    }

    #[test]
    fn init_schema_is_idempotent() {
        let conn = fresh_conn();
        conn.init_schema().expect("first init failed");
        // Second invocation should not error on the node tables (they're skipped
        // as "already exists" warnings).
        conn.init_schema().expect("second init failed");
    }

    #[test]
    fn init_schema_returns_report_with_skip_stats() {
        // AC: init_schema returns a SchemaInitReport exposing skipped_count and
        // skipped_reasons. On a fresh DB the 18 CREATE INDEX statements plus
        // the 3 FTS and 1 VECTOR index statements are unsupported by
        // LadybugDB's parser, so skipped_count must be > 0 and
        // skipped_reasons must mirror it.
        let conn = StorageConnection::in_memory().expect("open");
        let report = conn.init_schema().expect("init_schema should succeed");

        assert!(
            report.skipped_count > 0,
            "expected at least one skipped DDL statement (CREATE INDEX), got 0"
        );
        assert_eq!(
            report.skipped_reasons.len(),
            report.skipped_count as usize,
            "skipped_reasons length must match skipped_count"
        );
        // Each reason should mention the statement and the error.
        for reason in &report.skipped_reasons {
            assert!(
                reason.starts_with('`'),
                "reason should start with the statement in backticks: {reason}"
            );
        }
    }

    #[test]
    fn init_schema_records_unsupported_and_continues() {
        // AC: when a DDL statement fails with an "unsupported" error (parser
        // exception, already exists, does not exist, not supported), the
        // statement is recorded in the report and init_schema does NOT error.
        let conn = StorageConnection::in_memory().expect("open");

        // First init: CREATE INDEX statements fail with "Parser exception".
        let first = conn.init_schema().expect("first init should succeed");
        let first_skipped = first.skipped_count;
        assert!(
            first_skipped > 0,
            "first init should skip unsupported CREATE INDEX statements"
        );

        // Second init: tables now fail with "already exists" — also skipped.
        let second = conn.init_schema().expect("second init should succeed");
        assert!(
            second.skipped_count > first_skipped,
            "second init should skip more statements (tables already exist): \
             first={first_skipped}, second={}",
            second.skipped_count
        );
    }

    #[test]
    fn init_schema_returns_error_on_real_failure() {
        // AC: when a DDL statement fails with an error that is NOT an
        // "unsupported" signal (not supported / already exists / does not
        // exist / Parser exception), init_schema returns StorageError::Schema.
        let conn = StorageConnection::in_memory().expect("open");

        // NOTAREALTYPE triggers a "Catalog exception" — a genuine schema error
        // that is none of the unsupported signals above.
        let bad_ddl = vec![
            "CREATE NODE TABLE Project (id STRING, PRIMARY KEY (id));".to_string(),
            "CREATE NODE TABLE BadTbl (id NOTAREALTYPE, PRIMARY KEY (id));".to_string(),
        ];
        let result = conn.run_init_ddl(&bad_ddl);
        assert!(
            result.is_err(),
            "a real DDL failure must return an error, not be skipped"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(err, StorageError::Schema(_)),
            "expected StorageError::Schema, got {err:?}"
        );
        assert!(
            err.to_string().contains("BadTbl"),
            "error should mention the failing statement: {err}"
        );
    }

    #[test]
    fn execute_runs_ddl_and_dml() {
        let conn = fresh_conn();
        conn.init_schema().unwrap();
        conn.execute("CREATE (:Project {id: 'p1', name: 'demo', rootPath: '/', language: 'rust', fileCount: 0, indexedAt: 0});")
            .expect("execute create failed");
        let rows = conn
            .query("MATCH (p:Project) RETURN p.name AS name;")
            .expect("query failed");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], serde_json::json!("demo"));
    }

    #[test]
    fn query_returns_empty_for_no_rows() {
        let conn = fresh_conn();
        conn.init_schema().unwrap();
        let rows = conn
            .query("MATCH (p:Project) RETURN p.name AS name;")
            .expect("query failed");
        assert!(rows.is_empty());
    }

    #[test]
    fn query_returns_multiple_rows() {
        let conn = fresh_conn();
        conn.init_schema().unwrap();
        conn.execute("CREATE (:Project {id: 'a', name: 'alpha', rootPath: '/', language: 'c', fileCount: 0, indexedAt: 0});").unwrap();
        conn.execute("CREATE (:Project {id: 'b', name: 'beta', rootPath: '/', language: 'c', fileCount: 0, indexedAt: 0});").unwrap();
        let rows = conn
            .query("MATCH (p:Project) RETURN p.name AS name ORDER BY p.name;")
            .expect("query failed");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], serde_json::json!("alpha"));
        assert_eq!(rows[1][0], serde_json::json!("beta"));
    }

    #[test]
    fn query_with_columns_returns_names() {
        let conn = fresh_conn();
        conn.init_schema().unwrap();
        conn.execute("CREATE (:Project {id: 'a', name: 'alpha', rootPath: '/', language: 'c', fileCount: 0, indexedAt: 0});").unwrap();
        let (cols, rows) = conn
            .query_with_columns("MATCH (p:Project) RETURN p.name AS name, p.id AS id;")
            .expect("query failed");
        assert_eq!(cols, vec!["name", "id"]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], serde_json::json!("alpha"));
    }

    #[test]
    fn execute_invalid_query_returns_error() {
        let conn = fresh_conn();
        let err = conn.execute("MATCH (a:Person RETURN a.name;");
        assert!(err.is_err());
    }

    #[test]
    fn value_to_json_converts_int64() {
        let v = value_to_json(Value::Int64(42));
        assert_eq!(v, serde_json::json!(42));
    }

    #[test]
    fn value_to_json_converts_string() {
        let v = value_to_json(Value::String("hello".to_string()));
        assert_eq!(v, serde_json::json!("hello"));
    }

    #[test]
    fn value_to_json_converts_bool() {
        let v = value_to_json(Value::Bool(true));
        assert_eq!(v, serde_json::json!(true));
    }

    #[test]
    fn value_to_json_converts_double() {
        let v = value_to_json(Value::Double(2.5));
        assert_eq!(v, serde_json::json!(2.5));
    }

    #[test]
    fn value_to_json_converts_null() {
        let v = value_to_json(Value::Null(lbug::LogicalType::String));
        assert_eq!(v, serde_json::Value::Null);
    }

    #[test]
    fn value_to_json_converts_list() {
        let v = value_to_json(Value::List(
            lbug::LogicalType::Int64,
            vec![Value::Int64(1), Value::Int64(2)],
        ));
        assert_eq!(v, serde_json::json!([1, 2]));
    }

    #[test]
    fn with_connection_allows_prepared_statements() {
        let conn = fresh_conn();
        conn.init_schema().unwrap();
        let result = conn.with_connection(|c| {
            let mut stmt = c.prepare("RETURN $x AS x;")?;
            let mut result = c.execute(&mut stmt, vec![("x", Value::Int64(7))])?;
            let row = result.next().expect("expected one row");
            assert_eq!(row[0], Value::Int64(7));
            Ok(())
        });
        assert!(result.is_ok());
    }

    #[test]
    fn debug_does_not_panic() {
        let conn = fresh_conn();
        let s = format!("{conn:?}");
        assert!(s.contains("StorageConnection"));
    }

    #[test]
    fn value_to_json_converts_int8() {
        assert_eq!(value_to_json(Value::Int8(-1)), serde_json::json!(-1));
    }

    #[test]
    fn value_to_json_converts_int16() {
        assert_eq!(value_to_json(Value::Int16(256)), serde_json::json!(256));
    }

    #[test]
    fn value_to_json_converts_int32() {
        assert_eq!(value_to_json(Value::Int32(70_000)), serde_json::json!(70000));
    }

    #[test]
    fn value_to_json_converts_uint8() {
        assert_eq!(value_to_json(Value::UInt8(200)), serde_json::json!(200));
    }

    #[test]
    fn value_to_json_converts_uint16() {
        assert_eq!(value_to_json(Value::UInt16(40000)), serde_json::json!(40000));
    }

    #[test]
    fn value_to_json_converts_uint32() {
        assert_eq!(
            value_to_json(Value::UInt32(3_000_000)),
            serde_json::json!(3000000)
        );
    }

    #[test]
    fn value_to_json_converts_uint64_to_string() {
        // u64 values exceeding i64::MAX are serialized as strings to avoid
        // JSON integer overflow.
        let v = value_to_json(Value::UInt64(u64::MAX));
        assert_eq!(v, serde_json::json!(u64::MAX.to_string()));
    }

    #[test]
    fn value_to_json_converts_int128_to_string() {
        let v = value_to_json(Value::Int128(i128::MAX));
        assert_eq!(v, serde_json::json!(i128::MAX.to_string()));
    }

    #[test]
    fn value_to_json_converts_float() {
        let v = value_to_json(Value::Float(1.5));
        assert_eq!(v, serde_json::json!(1.5));
    }

    #[test]
    fn value_to_json_converts_json() {
        let v = value_to_json(Value::Json(serde_json::json!({"key": 1})));
        assert_eq!(v, serde_json::json!({"key": 1}));
    }

    #[test]
    fn value_to_json_converts_blob_to_string() {
        let v = value_to_json(Value::Blob(b"hello".to_vec()));
        assert_eq!(v, serde_json::json!("hello"));
    }

    #[test]
    fn value_to_json_converts_date() {
        use time::macros::date;
        let v = value_to_json(Value::Date(date!(2023 - 06 - 13)));
        assert_eq!(v, serde_json::json!("2023-06-13"));
    }

    #[test]
    fn value_to_json_converts_timestamp_variants() {
        use time::macros::datetime;
        let ts = datetime!(2023-06-13 11:25:30 UTC);
        assert!(value_to_json(Value::Timestamp(ts)).is_string());
        assert!(value_to_json(Value::TimestampTz(ts)).is_string());
        assert!(value_to_json(Value::TimestampNs(ts)).is_string());
        assert!(value_to_json(Value::TimestampMs(ts)).is_string());
        assert!(value_to_json(Value::TimestampSec(ts)).is_string());
    }

    #[test]
    fn value_to_json_converts_interval() {
        let v = value_to_json(Value::Interval(time::Duration::hours(5)));
        assert!(v.is_string());
    }

    #[test]
    fn value_to_json_converts_uuid() {
        let u = uuid::Uuid::nil();
        let v = value_to_json(Value::UUID(u));
        assert_eq!(v, serde_json::json!(u.to_string()));
    }

    #[test]
    fn value_to_json_converts_decimal() {
        let d = rust_decimal::Decimal::from_i128_with_scale(1234, 2); // 12.34
        let v = value_to_json(Value::Decimal(d));
        assert_eq!(v, serde_json::json!("12.34"));
    }

    #[test]
    fn value_to_json_converts_internal_id() {
        let id = lbug::InternalID {
            table_id: 3,
            offset: 7,
        };
        let v = value_to_json(Value::InternalID(id));
        assert_eq!(v, serde_json::json!("3:7"));
    }

    #[test]
    fn value_to_json_converts_array() {
        let v = value_to_json(Value::Array(
            lbug::LogicalType::Int64,
            vec![Value::Int64(1), Value::Int64(2)],
        ));
        assert_eq!(v, serde_json::json!([1, 2]));
    }

    #[test]
    fn value_to_json_converts_struct() {
        let v = value_to_json(Value::Struct(vec![
            ("name".to_string(), Value::String("Alice".to_string())),
            ("age".to_string(), Value::Int64(25)),
        ]));
        let obj = v.as_object().expect("should be object");
        assert_eq!(obj["name"], serde_json::json!("Alice"));
        assert_eq!(obj["age"], serde_json::json!(25));
    }

    #[test]
    fn value_to_json_converts_map() {
        let v = value_to_json(Value::Map(
            (lbug::LogicalType::String, lbug::LogicalType::Int64),
            vec![(Value::String("key".to_string()), Value::Int64(24))],
        ));
        let obj = v.as_object().expect("should be object");
        assert_eq!(obj["key"], serde_json::json!(24));
    }

    #[test]
    fn value_to_json_converts_node() {
        let node = lbug::NodeVal::new(
            lbug::InternalID {
                table_id: 0,
                offset: 0,
            },
            "Person",
        );
        let v = value_to_json(Value::Node(node));
        assert!(v.is_string());
    }

    #[test]
    fn value_to_json_converts_rel() {
        let rel = lbug::RelVal::new(
            lbug::InternalID {
                table_id: 0,
                offset: 0,
            },
            lbug::InternalID {
                table_id: 1,
                offset: 0,
            },
            "knows",
        );
        let v = value_to_json(Value::Rel(rel));
        assert!(v.is_string());
    }

    #[test]
    fn value_to_json_converts_recursive_rel() {
        let node = lbug::NodeVal::new(
            lbug::InternalID {
                table_id: 0,
                offset: 0,
            },
            "Person",
        );
        let rel = lbug::RelVal::new(
            lbug::InternalID {
                table_id: 0,
                offset: 0,
            },
            lbug::InternalID {
                table_id: 1,
                offset: 0,
            },
            "knows",
        );
        let v = value_to_json(Value::RecursiveRel {
            nodes: vec![node],
            rels: vec![rel],
        });
        let obj = v.as_object().expect("should be object");
        assert!(obj.contains_key("nodes"));
        assert!(obj.contains_key("rels"));
    }

    #[test]
    fn value_to_json_converts_union() {
        let v = value_to_json(Value::Union {
            types: vec![("Num".to_string(), lbug::LogicalType::Int8)],
            value: Box::new(Value::Int8(42)),
        });
        assert_eq!(v, serde_json::json!(42));
    }

    // --- DatabaseCorrupt detection (complete-test-coverage spec) ---

    /// Verifies that opening a file with invalid bytes (not a valid LadybugDB
    /// file) is detected as corruption and mapped to
    /// `IndexError::DatabaseCorrupt` (exit code 4).
    #[test]
    fn database_corrupt_detected_on_malformed_db() {
        use crate::index::IndexError;

        let dir = tempfile::tempdir().expect("tempdir");
        let lbug_file = dir.path().join("corrupt.lbug");
        std::fs::write(&lbug_file, b"this is not a valid ladybugdb file")
            .expect("write corrupt file");
        // Leak the tempdir so the database files survive for the test's
        // lifetime (LadybugDB keeps file handles open).
        std::mem::forget(dir);

        let result = StorageConnection::open(&lbug_file).map_err(IndexError::from);
        match result {
            Err(IndexError::DatabaseCorrupt(_)) => (), // 通过
            Err(e) => panic!(
                "期望 IndexError::DatabaseCorrupt，实际 {:?}（消息：{}）",
                e, e
            ),
            Ok(_) => panic!("期望错误，实际成功打开损坏数据库"),
        }
    }

    /// Verifies that "database is locked" is NOT detected as corruption —
    /// locking is a transient condition handled by retry logic, not corruption.
    #[test]
    fn database_locked_not_detected_as_corrupt() {
        let locked_err = StorageError::Query("database is locked".to_string());
        assert!(
            !is_corruption_error(&locked_err),
            "database is locked 不应被检测为损坏"
        );
    }

    #[test]
    fn value_to_json_map_with_non_string_key() {
        // Cover the `other => other.to_string()` arm of value_to_json: a Map
        // entry whose key is not a Value::String (e.g. Int64) is stringified.
        let val = Value::Map(
            (lbug::LogicalType::Int64, lbug::LogicalType::Int64),
            vec![(Value::Int64(42), Value::Int64(100))],
        );
        let json = value_to_json(val);
        let obj = json.as_object().expect("should be object");
        assert_eq!(obj.get("42"), Some(&serde_json::json!(100)));
    }
}
