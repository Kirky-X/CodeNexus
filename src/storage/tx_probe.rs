// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Regression tests for `StorageConnection::in_write_transaction` — the
//! LoadPhase atomicity primitive. Guards two properties discovered when the
//! transaction boundary was introduced:
//!
//! 1. Transaction state is per-connection: the BEGIN/COMMIT pair (and every
//!    staged statement) must run on ONE dedicated connection — the original
//!    statement-per-connection `execute` silently broke this ("No active
//!    transaction for COMMIT").
//! 2. `COPY FROM` statements join the transaction: node/edge bulk loads must
//!    roll back together with DELETEs when the closure fails.

use crate::model::{Node, NodeLabel};
use crate::storage::capability::Storage;
use crate::storage::repository::Repository;

#[test]
fn in_write_transaction_survives_delete_and_copy() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("tx_probe");
    let repo = Repository::open(&db).unwrap();
    repo.init_schema().unwrap();

    // Seed one node through the normal (statement-per-connection) path.
    let node = Node::builder(NodeLabel::File, "probe/a.rs", "probe/a.rs")
        .id("file_probe1")
        .project("p_probe")
        .build();
    repo.save_nodes_stream(std::iter::once(&node), NodeLabel::File)
        .unwrap();

    // Delete + COPY inside one transaction, then succeed → both applied.
    let result: Result<(), crate::storage::error::StorageError> = repo.in_write_transaction(|tx| {
        tx.execute("MATCH (n:File) WHERE n.id = 'file_probe1' DELETE n;")?;
        let replacement = Node::builder(NodeLabel::File, "probe/b.rs", "probe/b.rs")
            .id("file_probe2")
            .project("p_probe")
            .build();
        Repository::save_nodes_stream_on(tx, std::iter::once(&replacement), NodeLabel::File)
    });
    result.expect("committed transaction");

    let rows = repo.query("MATCH (n:File) RETURN n.id AS id;").unwrap();
    let ids: Vec<String> = rows
        .iter()
        .filter_map(|r| r.first().and_then(|v| v.as_str()).map(String::from))
        .collect();
    assert!(
        !ids.contains(&"file_probe1".to_string()),
        "DELETE inside tx must be visible after commit, got {ids:?}"
    );
    assert!(
        ids.contains(&"file_probe2".to_string()),
        "COPY inside tx must be visible after commit, got {ids:?}"
    );
}

#[test]
fn in_write_transaction_rolls_back_on_err() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("tx_probe_rollback");
    let repo = Repository::open(&db).unwrap();
    repo.init_schema().unwrap();

    let node = Node::builder(NodeLabel::File, "probe/c.rs", "probe/c.rs")
        .id("file_probe3")
        .project("p_probe")
        .build();

    // Closure stages a write then fails → ROLLBACK must undo it.
    let result: Result<(), crate::storage::error::StorageError> = repo.in_write_transaction(|tx| {
        Repository::save_nodes_stream_on(tx, std::iter::once(&node), NodeLabel::File)?;
        Err(crate::storage::error::StorageError::InvalidData(
            "deliberate failure".to_string(),
        ))
    });
    assert!(result.is_err(), "closure error must propagate");

    let rows = repo
        .query("MATCH (n:File) WHERE n.id = 'file_probe3' RETURN count(n.id) AS cnt;")
        .unwrap();
    let cnt = rows
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    assert_eq!(cnt, 0, "staged COPY must be rolled back");
}

#[test]
fn probe_batch_delete_in_tx() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("tx_probe_batch");
    let repo = Repository::open(&db).unwrap();
    repo.init_schema().unwrap();

    let node = Node::builder(NodeLabel::File, "probe/d.rs", "probe/d.rs")
        .id("file_probe4")
        .project("p_probe")
        .build();
    repo.save_nodes_stream(std::iter::once(&node), NodeLabel::File)
        .unwrap();

    let result: Result<(), crate::storage::error::StorageError> = repo.in_write_transaction(|tx| {
        Repository::delete_file_nodes_batch_on(tx, &["probe/d.rs".to_string()], "p_probe")
    });
    println!("[probe-batch] result: {:?}", result.as_ref().map(|_| "ok"));
    assert!(
        result.is_ok(),
        "batch delete in tx failed: {:?}",
        result.err()
    );

    // Verify the deletion actually took effect (not just "no error").
    use crate::storage::capability::Storage;
    let rows = repo
        .query("MATCH (n:Function) WHERE n.filePath = 'probe/d.rs' RETURN count(n.id) AS cnt;")
        .unwrap();
    let cnt = rows
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    println!("[probe-batch] function rows after committed delete-tx: {cnt}");
    assert_eq!(cnt, 0, "committed delete-tx must remove the rows");
}

#[test]
fn probe_which_statement_kills_tx() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("tx_probe_stmt");
    let repo = Repository::open(&db).unwrap();
    repo.init_schema().unwrap();

    // a) SELECT inside tx
    let r: Result<(), crate::storage::error::StorageError> = repo.in_write_transaction(|tx| {
        tx.query("MATCH (n:File) WHERE n.filePath IN ['nope'] RETURN n.id AS id;")?;
        Ok(())
    });
    println!("[stmt] SELECT in tx: {:?}", r.is_ok());

    // b) DELETE inside tx
    let r: Result<(), crate::storage::error::StorageError> = repo.in_write_transaction(|tx| {
        tx.execute("MATCH (n:File) WHERE n.filePath IN ['nope'] AND n.project = 'p' DELETE n;")?;
        Ok(())
    });
    println!("[stmt] DELETE in tx: {:?}", r.is_ok());

    // c) DELETE + second DELETE
    let r: Result<(), crate::storage::error::StorageError> = repo.in_write_transaction(|tx| {
        tx.execute("MATCH (n:File) WHERE n.filePath IN ['nope'] AND n.project = 'p' DELETE n;")?;
        tx.execute(
            "MATCH (r:CodeRelation) WHERE r.source IN ['x'] OR r.target IN ['x'] DELETE r;",
        )?;
        Ok(())
    });
    println!("[stmt] DELETE+DELETE in tx: {:?}", r.is_ok());

    // d) SELECT on a label WITHOUT filePath column (Process) — the binder error case
    let r: Result<(), crate::storage::error::StorageError> = repo.in_write_transaction(|tx| {
        tx.query("MATCH (n:Process) WHERE n.filePath IN ['nope'] RETURN n.id AS id;")?;
        Ok(())
    });
    println!("[stmt] SELECT-on-no-filePath-label in tx: {:?}", r.is_ok());
}

#[test]
fn probe_show_tables_columns() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("tx_probe_tables");
    let repo = Repository::open(&db).unwrap();
    repo.init_schema().unwrap();
    let rows = repo.query("CALL show_tables() RETURN *;").unwrap();
    println!("[tables] row count: {}", rows.len());
    for r in rows.iter().take(3) {
        println!("[tables] row: {r:?}");
    }
}

/// Enabler test for full LoadPhase atomicity: lbug 0.20 **allows**
/// re-inserting a primary key deleted earlier in the same transaction (the
/// row is replaced). This is what lets `LoadPhase` wrap delete+insert in ONE
/// transaction despite the Project id and changed-file FQN node ids being
/// re-inserted verbatim.
///
/// **When this test fails, the engine regression breaks index atomicity** —
/// `LoadPhase`'s transaction scope must be reduced again.
#[test]
fn lbug_allows_delete_then_insert_same_pk_in_one_tx() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("tx_probe_pk_limit");
    let repo = Repository::open(&db).unwrap();
    repo.init_schema().unwrap();

    let result: Result<(), crate::storage::error::StorageError> = repo.in_write_transaction(|tx| {
        // Insert, then delete, then re-insert the SAME primary key.
        tx.execute(
            "CREATE (:File {id: 'pk_probe', project: 'p', name: 'a.rs', \
                 filePath: 'a.rs', language: 'rust', hash: 'h', lineCount: 1});",
        )?;
        tx.execute("MATCH (n:File) WHERE n.id = 'pk_probe' DELETE n;")?;
        tx.execute(
            "CREATE (:File {id: 'pk_probe', project: 'p', name: 'a.rs', \
                 filePath: 'a.rs', language: 'rust', hash: 'h2', lineCount: 2});",
        )
    });

    result.expect(
        "delete+re-insert of the same PK in one tx must succeed; \
         if this fails, LoadPhase full atomicity is broken",
    );
}
