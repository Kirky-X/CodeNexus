// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! 文件监视守护：`notify` 防抖 → 增量索引（Observer 模式）→ 优雅停止。
//!
//! 覆盖 CLI：`daemon`。

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use codenexus::daemon::{Daemon, IndexObserver};
use codenexus::index::IndexFacade;
use codenexus::query::QueryFacade;
use codenexus_examples::setup;

const SAMPLE_CODE: &str = "pub fn initial() -> u32 { 0 }\n";

fn main() {
    println!("=== CodeNexus Example: Daemon Watch (daemon) ===\n");

    let ctx = setup(SAMPLE_CODE);

    // The observer owns a facade bound to the same database file.
    let facade = IndexFacade::new(&ctx.db_path).expect("IndexFacade::new");
    let mut daemon = Daemon::new(&ctx.source_dir, "demo", 300, &ctx.db_path);
    daemon.add_observer(Box::new(IndexObserver::new(
        facade,
        "demo".to_string(),
        ctx.source_dir.clone(),
    )));

    let stop = daemon.stop_handle();
    let stop_bg = Arc::clone(&stop);
    let watch_dir = ctx.source_dir.clone();
    let writer = thread::spawn(move || {
        // Let the watcher arm itself, then drop in a new file to trigger
        // a debounced incremental index.
        thread::sleep(Duration::from_millis(600));
        std::fs::write(
            watch_dir.join("extra.rs"),
            "pub fn added_by_daemon() -> u32 { 7 }\n",
        )
        .expect("write extra file");
        println!("[watcher] wrote extra.rs");
        thread::sleep(Duration::from_millis(1200));
        stop_bg.store(true, Ordering::SeqCst);
        println!("[watcher] stop flag set");
    });

    println!("=== Watching {:?} (debounce=300ms) ===", ctx.source_dir);
    daemon.run().expect("daemon run");

    writer.join().expect("writer thread");
    let _ = stop; // keep the handle alive until run() returns

    // The observer indexed the new file incrementally — verify via query.
    let query = QueryFacade::new(&ctx.db_path).expect("QueryFacade::new");
    let qr = query
        .cypher("MATCH (f:Function) WHERE f.name = 'added_by_daemon' RETURN f.name")
        .expect("cypher");
    println!("\n=== Incremental Index Result ===");
    println!("  functions named added_by_daemon: {}", qr.rows.len());
}
