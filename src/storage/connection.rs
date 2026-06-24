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
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let db = Database::new(path, SystemConfig::default())?;
        Ok(Self { db })
    }

    /// Creates an in-memory database (alias for `open(":memory:")`).
    pub fn in_memory() -> Result<Self> {
        Self::open(":memory:")
    }

    /// Initializes the full CodeNexus schema: all 20 node tables, the
    /// `CodeRelation` table, the optional `Embedding` table, and all secondary
    /// indexes (DDD §12.1, §12.2).
    ///
    /// Statements that LadybugDB cannot execute (e.g. unsupported index types
    /// or the optional `Embedding` table when the VECTOR extension is absent)
    /// are logged as warnings and skipped rather than aborting initialization.
    pub fn init_schema(&self) -> Result<()> {
        for stmt in all_init_ddl() {
            if let Err(err) = self.execute(&stmt) {
                // Indexes and the optional Embedding table may be unsupported
                // by the linked LadybugDB build; skip them with a warning.
                let msg = err.to_string();
                let is_optional = stmt.starts_with("CREATE INDEX")
                    || stmt.contains("Embedding")
                    || msg.contains("already exists");
                if is_optional {
                    warn!(statement = %stmt, error = %msg, "skipping optional DDL statement");
                } else {
                    return Err(StorageError::Schema(format!(
                        "failed to execute DDL `{stmt}`: {msg}"
                    )));
                }
            }
        }
        Ok(())
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
}
