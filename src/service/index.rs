// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Index command: execute the full index pipeline.

use std::path::Path;

use serde::Serialize;

use crate::index::IndexResult;

#[cfg(any(feature = "cli", feature = "mcp", test))]
use crate::kit::{AsyncKit, AsyncReady, IndexerModule};
#[cfg(any(feature = "cli", feature = "mcp", feature = "lsp", test))]
use crate::service::error::CodeNexusError;
#[cfg(any(feature = "cli", feature = "mcp", feature = "lsp", test))]
use crate::storage::{QualityChecker, Repository};

#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::error::{kit_not_initialized, to_api_error, wrap_kit_error};
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::runtime::kit;
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::storage::StorageConfig;

#[cfg(feature = "lsp")]
use crate::lsp::LspProvider;
#[cfg(feature = "cli")]
use sdforge::forge;
#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;

/// JSON-serializable view of [`IndexResult`].
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct IndexOutput {
    pub project_id: String,
    pub files_indexed: usize,
    pub files_skipped: usize,
    pub nodes_created: usize,
    pub edges_created: usize,
    pub duration_ms: u64,
}

impl From<IndexResult> for IndexOutput {
    fn from(r: IndexResult) -> Self {
        Self {
            project_id: r.project_id,
            files_indexed: r.files_indexed,
            files_skipped: r.files_skipped,
            nodes_created: r.nodes_created,
            edges_created: r.edges_created,
            duration_ms: r.duration_ms,
        }
    }
}

/// Maximum number of LSP providers that can be wired in [`build_lsp_providers`].
///
/// This const bounds the concurrency of [`start_active_providers_parallel`]:
/// that function spawns one thread per active provider without a semaphore,
/// relying on the bound being small (≤8) and LSP startup being IO-bound.
/// Adding a 9th provider without raising this const would cause the assert
/// in `build_lsp_providers` to fire (arch-review LOW-3).
#[cfg(feature = "lsp")]
const MAX_LSP_PROVIDERS: usize = 8;

#[cfg(feature = "lsp")]
fn build_lsp_providers() -> Vec<(&'static str, Box<dyn LspProvider>)> {
    use crate::lsp::{
        ClangdClient, FortlsClient, GoplsClient, JdtlsClient, PyrightClient, RustAnalyzerClient,
        TypeScriptLanguageClient,
    };

    let providers: Vec<(&'static str, Box<dyn LspProvider>)> = vec![
        ("rs", Box::new(RustAnalyzerClient::new())),
        ("py", Box::new(PyrightClient::new())),
        ("c", Box::new(ClangdClient::new())),
        ("cpp", Box::new(ClangdClient::new())),
        ("go", Box::new(GoplsClient::new())),
        ("ts", Box::new(TypeScriptLanguageClient::new())),
        ("f90", Box::new(FortlsClient::new())),
        ("java", Box::new(JdtlsClient::new())),
    ];
    // arch-review LOW-3: assert the semaphore-free bound expected by
    // `start_active_providers_parallel`. Adding a new provider without
    // raising MAX_LSP_PROVIDERS will trip this assert at startup.
    assert!(
        providers.len() <= MAX_LSP_PROVIDERS,
        "MAX_LSP_PROVIDERS={} but {} providers wired; raise the const or remove a provider",
        MAX_LSP_PROVIDERS,
        providers.len()
    );
    providers
}

#[cfg(feature = "lsp")]
fn build_symbol_queries(project: &str) -> [String; 2] {
    use crate::storage::schema::escape_cypher_string;

    let proj = escape_cypher_string(project);
    // P-09: pre-merge optimization kept as two separate queries because
    // LadybugDB's Cypher subset does NOT support `UNION` / `UNION ALL`
    // (see `src/analysis/architecture.rs` line ~38: "LadybugDB does not
    // support `UNION`, so we issue one query per table and merge in Rust").
    // A single `MATCH (n) WHERE n:Function OR n:Method ...` would also not
    // work — LadybugDB does not support multi-label `WHERE (n:A OR n:B)`
    // expressions either (same source). The two queries are issued
    // sequentially and their result rows are merged in Rust via
    // `rows.extend(r)` in `enhance_with_lsp`. The per-query overhead is one
    // round-trip to the embedded LadybugDB process — negligible compared to
    // the per-symbol LSP `hover` cost that dominates this function.
    [
        format!(
            "MATCH (n:Function) WHERE n.project = '{proj}' \
             AND n.filePath IS NOT NULL AND n.startLine IS NOT NULL \
             RETURN n.id AS id, n.filePath AS filePath, n.startLine AS startLine;"
        ),
        format!(
            "MATCH (n:Method) WHERE n.project = '{proj}' \
             AND n.filePath IS NOT NULL AND n.startLine IS NOT NULL \
             RETURN n.id AS id, n.filePath AS filePath, n.startLine AS startLine;"
        ),
    ]
}

#[cfg(feature = "lsp")]
fn select_provider_for_ext<'a>(
    ext_map: &std::collections::HashMap<&'static str, &'a dyn LspProvider>,
    ext: &str,
) -> Option<&'a dyn LspProvider> {
    // P-02: HashMap O(1) lookup replaces the pre-P-02 linear `.iter().find`
    // over 8 providers that ran once per symbol (100k+ symbols → 100k+ scans).
    // M2/P-07: returns `None` for unknown extensions instead of falling back
    // to `providers[0]` — callers (the hover loop) skip symbols whose file
    // extension is not wired to a real LSP server (e.g. .md, .toml). Pre-M2
    // started rust-analyzer for .md files, wasting ~300 MB-2 GB of RSS.
    ext_map.get(ext).copied()
}

#[cfg(feature = "lsp")]
fn build_semantic_type_update(id: &str, project: &str, text: &str) -> String {
    use crate::storage::schema::escape_cypher_string;

    format!(
        "MATCH (n {{id: '{id}', project: '{proj}'}}) \
         SET n.semantic_type = '{sem}';",
        id = escape_cypher_string(id),
        proj = escape_cypher_string(project),
        sem = escape_cypher_string(text),
    )
}

/// Batch size for LSP hover `semantic_type` UPDATE statements (P-01).
///
/// 500 rows per UNWIND batch balances round-trip reduction against statement
/// size. LadybugDB's Cypher parser has no hard limit on UNWIND list length,
/// but extremely long statements (>1 MB) hit the embedded channel's buffer
/// threshold and degrade throughput. 500 rows × ~80 bytes per `{id, sem}` map
/// ≈ 40 KB per statement — well within the buffer.
#[cfg(feature = "lsp")]
const LSP_HOVER_BATCH_SIZE: usize = 500;

/// Builds a single Cypher `UNWIND ... MATCH ... SET` statement that updates
/// `semantic_type` for all `(id, sem)` pairs in `batch` in one round-trip
/// (P-01 — batch UPDATE replaces per-symbol `execute`).
///
/// Returns an empty `String` when `batch` is empty so the caller can skip
/// the `execute` call entirely (saves one round-trip on tail flush when the
/// total symbol count is an exact multiple of [`LSP_HOVER_BATCH_SIZE`]).
///
/// # Statement shape
///
/// ```cypher
/// UNWIND [
///   {id: 'sym1', sem: 'fn foo() -> Result<T, E>'},
///   {id: 'sym2', sem: 'struct Bar { x: i32 }'},
///   ...
/// ] AS row
/// MATCH (n {id: row.id, project: 'my-project'})
/// SET n.semantic_type = row.sem;
/// ```
///
/// # Escaping
///
/// `id`, `sem`, and `project` are all escaped via [`escape_cypher_string`].
/// Single quotes become `\'`, backslashes are doubled, control chars become
/// `\n`/`\r`/`\t`. Without escaping the Cypher string literal would terminate
/// early on any embedded quote and the statement would be a syntax error.
///
/// # Why UNWIND instead of multi-`SET`
///
/// LadybugDB's Cypher subset does not support `CASE WHEN` expressions or
/// multi-row `SET` clauses outside of `UNWIND`. The `UNWIND [{...}, {...}] AS
/// row` form is the only way to update N rows with N different values in a
/// single statement. Each list element is a Cypher map; `row.id` and
/// `row.sem` are field accesses on the aliased row variable.
///
/// # Why not `UNION`
///
/// `UNION` would also batch updates but LadybugDB does not support `UNION` /
/// `UNION ALL` in its Cypher subset (see [`build_symbol_queries`]'s P-09
/// comment). `UNWIND` is supported and is the idiomatic batch form.
///
/// [`escape_cypher_string`]: crate::storage::schema::escape_cypher_string
#[cfg(feature = "lsp")]
fn build_batch_semantic_type_update(batch: &[(String, String)], project: &str) -> String {
    use crate::storage::schema::escape_cypher_string;
    use std::fmt::Write as _;

    if batch.is_empty() {
        return String::new();
    }
    let proj = escape_cypher_string(project);
    // Dynamic capacity estimate (perf-review M-03): 16 bytes fixed overhead
    // per `{id: '', sem: ''}` map + actual id/sem lengths (post-escape).
    // Pre-size to avoid reallocation; overshoot by ~10% for separator bytes.
    let estimated: usize = batch
        .iter()
        .map(|(id, sem)| id.len() + sem.len() + 16)
        .sum::<usize>()
        + batch.len()
        + 80; // UNWIND/MATCH/SET wrapper overhead
    let mut rows = String::with_capacity(estimated);
    for (i, (id, sem)) in batch.iter().enumerate() {
        if i > 0 {
            rows.push_str(", ");
        }
        // write! directly into `rows` (perf-review M-01): avoids the temporary
        // String allocation that `format!` would produce per entry. For 500
        // entries per batch this saves ~500 × ~100 B = ~50 KB of churn per
        // flush, compounding to ~10 MB across 100k symbols.
        let _ = write!(
            rows,
            "{{id: '{}', sem: '{}'}}",
            escape_cypher_string(id),
            escape_cypher_string(sem),
        );
    }
    format!(
        "UNWIND [{rows}] AS row \
         MATCH (n {{id: row.id, project: '{proj}'}}) \
         SET n.semantic_type = row.sem;"
    )
}

/// Flushes a batch of `(id, semantic_type)` pairs to the database (P-01).
///
/// First attempts a single batch `UNWIND ... SET` via [`build_batch_semantic_type_update`].
/// If the batch statement fails (e.g. LadybugDB's Cypher parser rejects the
/// `UNWIND` form, or a transient connection error), falls back to per-row
/// `execute(&build_semantic_type_update(...))` so that a single bad row does
/// not poison the entire batch — each successful per-row UPDATE increments
/// `*enhanced`, each failure increments `*skipped`.
///
/// The `exec` closure abstracts the SQL execution so this function is
/// unit-testable without a live LadybugDB connection. Production callers
/// pass `|q| repo.connection().execute(q)`; tests pass a counter closure
/// that records calls and returns canned `Result`s.
///
/// # Arguments
///
/// * `exec` - Closure that executes a Cypher statement. Called once for the
///   batch (on success) or once per row (on batch failure + fallback).
/// * `batch` - Mutable buffer of `(id, sem)` pairs. Always cleared on return.
/// * `project` - Project name for the `MATCH (n {id:..., project: '...'})`
///   clause. Escaped via [`escape_cypher_string`].
/// * `enhanced` - Counter incremented per successful UPDATE (batch or per-row).
/// * `skipped` - Counter incremented per failed per-row UPDATE (batch failure
///   itself does not increment skipped — only individual row failures do).
///
/// # Empty batch
///
/// Returns without calling `exec`. This lets callers unconditionally call
/// `flush_semantic_type_batch` after the hover loop without worrying about
/// the tail flush on an exact-multiple-of-batch-size symbol count.
///
/// # Batch success path
///
/// When the batch `execute` succeeds, `*enhanced += batch.len()` (all rows
/// updated in one statement). `exec` is called exactly once.
///
/// # Batch failure → per-row fallback (Rule 12: failure must be visible)
///
/// When the batch `execute` fails, an `[warn]` line is emitted to stderr
/// with the error detail so operators can diagnose why the batch path did
/// not fire (e.g. "LadybugDB version X does not support UNWIND"). The
/// function then iterates `batch` and calls
/// `exec(&build_semantic_type_update(id, project, sem))` per row. Each
/// success increments `*enhanced`, each failure increments `*skipped`. This
/// is O(N) round-trips but only triggers when the batch path is unsupported
/// — once LadybugDB stabilizes `UNWIND` support the fallback becomes dead
/// code (defensive, kept for forward compatibility).
///
/// [`escape_cypher_string`]: crate::storage::schema::escape_cypher_string
#[cfg(feature = "lsp")]
fn flush_semantic_type_batch<F>(
    exec: F,
    batch: &mut Vec<(String, String)>,
    project: &str,
    enhanced: &mut u32,
    skipped: &mut u32,
) where
    F: Fn(&str) -> crate::storage::Result<()>,
{
    if batch.is_empty() {
        return;
    }
    let processed = batch.len();
    let stmt = build_batch_semantic_type_update(batch, project);
    // Defensive: empty statement only happens when batch was empty (already
    // returned above). If a future bug produces an empty statement for a
    // non-empty batch, skip exec and clear batch rather than calling exec("").
    // (arch-review LOW-5: `debug_assert` is compiled out in release, so the
    // explicit `if` is the production guard.)
    if stmt.is_empty() {
        debug_assert!(false, "non-empty batch must produce non-empty statement");
        batch.clear();
        return;
    }
    match exec(&stmt) {
        Ok(()) => {
            // Batch path: one round-trip updated all rows.
            // `processed` is bounded by `LSP_HOVER_BATCH_SIZE = 500` (compile-time
            // const), so `as u32` cannot overflow (perf-review L-02).
            *enhanced += processed as u32;
        }
        Err(e) => {
            // Rule 12: batch failure must be visible — emit warning with the
            // error detail so operators can diagnose why UNWIND path was skipped.
            // (arch-review MEDIUM-3: previously this branch was silent.)
            eprintln!(
                "[warn] P-01 batch UNWIND failed ({e:?}), falling back to per-row execute for {processed} symbols"
            );
            // Fallback: per-row execute. LadybugDB's UNWIND support is spotty across
            // versions; this path guarantees forward compatibility. Each row gets its
            // own build_semantic_type_update + execute, with per-row success/failure
            // accounting. The batch statement's failure is NOT counted as a skip —
            // only individual per-row failures are (a batch failure is an executor
            // issue, not a symbol issue).
            for (id, sem) in batch.iter() {
                let row_stmt = build_semantic_type_update(id, project, sem);
                if exec(&row_stmt).is_ok() {
                    *enhanced += 1;
                } else {
                    *skipped += 1;
                }
            }
        }
    }
    batch.clear();
}

/// Starts each LSP provider whose extension is in `active_exts` in parallel
/// using `std::thread::scope` (P-04).
///
/// Pre-P-04 the startup loop was serial: `for (ext, p) in &providers {
/// p.start(workspace) }`. For polyglot repos this summed per-provider startup
/// times (rust-analyzer ~3-8 s + clangd ~1-3 s + gopls ~2-5 s + … = ~20-40 s
/// worst case). `thread::scope` spawns one thread per active provider so all
/// `start()` calls run concurrently; the main thread blocks at scope exit
/// until all spawned threads have joined.
///
/// # Concurrency bound
///
/// No semaphore is used. At most 8 LSP providers can be active (rs, py, c,
/// cpp, go, ts, f90, java — the wired set in [`build_lsp_providers`]). LSP
/// startup is IO-bound (waiting on subprocess pipes for `initialize`/
/// `initialized` handshake), so 8 threads on an 8-core host does not
/// oversubscribe CPU. A semaphore would add complexity without benefit.
///
/// # Failure semantics
///
/// A provider whose `start()` returns `Err(LspError::ServerStart)` is logged
/// to stderr as a warning and counted as not started — the function
/// continues with the remaining providers (degraded mode, same as pre-P-04).
/// A provider whose `start()` **panics** is caught via
/// `std::panic::catch_unwind` inside the spawned thread; the panic is
/// converted to a `LspError::ServerStart` message and logged, so a buggy
/// provider cannot abort the entire scope (which would otherwise propagate
/// the panic to the main thread and abort `enhance_with_lsp`).
///
/// # Returns
///
/// The number of providers whose `start()` returned `Ok(())`. Always
/// `<= active_exts.len()`.
///
/// # Shutdown contract for panicked providers (arch-review MEDIUM-2)
///
/// When a provider's `start()` panics, the panic is caught via
/// `catch_unwind` and converted to `LspError::ServerStart`. The provider
/// instance is **not** destroyed — it remains in the `providers` Vec and
/// will be passed to `LspProvider::shutdown()` at the end of
/// `enhance_with_lsp`. The `LspProvider::shutdown` trait contract already
/// requires "safe to call on a client that was never successfully started";
/// callers MUST additionally ensure their `shutdown()` implementation is
/// safe to call after a `start()` that panicked mid-initialization (e.g.
/// the impl must not assume `Mutex<Option<Session>>` is in any specific
/// state, and must not unwrap `Option` fields that may not have been set).
/// Current `RustAnalyzerClient` / `PyrightClient` / `ClangdClient` /
/// `GoplsClient` / `TypeScriptLanguageClient` / `FortlsClient` /
/// `JdtlsClient` all use `Mutex<Option<Session>>` initialized to `None`
/// and `shutdown()` short-circuits on `None`, so panic-then-shutdown is
/// safe by construction today.
#[cfg(feature = "lsp")]
fn start_active_providers_parallel(
    providers: &[(&'static str, Box<dyn LspProvider>)],
    active_exts: &std::collections::HashSet<&'static str>,
    workspace: &Path,
) -> u32 {
    use crate::lsp::LspError;

    // Collect references to (ext, provider) pairs that need starting.
    // `&Box<dyn LspProvider>` coerces to `&dyn LspProvider`; the borrow is
    // `Send` because `dyn LspProvider: Sync` (trait bound).
    let active: Vec<(&'static str, &dyn LspProvider)> = providers
        .iter()
        .filter(|(ext, _)| active_exts.contains(ext))
        .map(|(ext, p)| (*ext, p.as_ref()))
        .collect();

    if active.is_empty() {
        return 0;
    }

    // Spawn one thread per active provider. `thread::scope` guarantees all
    // spawned threads are joined before the closure returns, so `results`
    // is fully populated when we read it.
    let results: Vec<(&'static str, Result<(), LspError>)> = std::thread::scope(|s| {
        let handles: Vec<_> = active
            .iter()
            .map(|(ext, provider)| {
                // `*provider` is `&dyn LspProvider`; the closure captures it
                // by reference. `&dyn LspProvider` is `Send` because
                // `dyn LspProvider: Sync`.
                s.spawn(move || {
                    // Catch panics so a buggy provider cannot abort the scope.
                    // `AssertUnwindSafe` is required because `&dyn LspProvider`
                    // is not `UnwindSafe` (trait objects never are). The panic
                    // payload is converted to a `LspError::ServerStart` message.
                    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        provider.start(workspace)
                    }));
                    match res {
                        Ok(outer) => (*ext, outer),
                        Err(payload) => {
                            let msg = payload
                                .downcast_ref::<String>()
                                .cloned()
                                .or_else(|| {
                                    payload
                                        .downcast_ref::<&'static str>()
                                        .map(|s| s.to_string())
                                })
                                .unwrap_or_else(|| "unknown panic payload".to_string());
                            (
                                *ext,
                                Err(LspError::ServerStart(format!("provider panicked: {msg}"))),
                            )
                        }
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("scope thread panicked unexpectedly"))
            .collect()
    });

    let mut started_count: u32 = 0;
    for (ext, result) in results {
        match result {
            Ok(()) => started_count += 1,
            Err(LspError::ServerStart(msg)) => {
                eprintln!("[warn] LSP server start failed for {ext} (degrading): {msg}");
            }
            Err(other) => {
                // start() only returns ServerStart per the trait contract, but
                // defensive: log any other error variant without aborting.
                eprintln!("[warn] LSP server start failed for {ext} (degrading): {other:?}");
            }
        }
    }
    started_count
}

/// Extracts `(id, file_path_str, start_line)` from a Cypher query result row.
/// Returns `None` if any field is missing or has the wrong type.
#[cfg(feature = "lsp")]
fn extract_lsp_row_fields(row: &[serde_json::Value]) -> Option<(String, String, u64)> {
    let id = row.first().and_then(|v| v.as_str())?.to_string();
    let file_path_str = row.get(1).and_then(|v| v.as_str())?.to_string();
    let start_line = row.get(2).and_then(|v| v.as_u64())?;
    Some((id, file_path_str, start_line))
}

/// Resolves a file path to an absolute path, joining with `workspace` if the
/// path is relative.
#[cfg(feature = "lsp")]
fn resolve_abs_file_path(workspace: &Path, file_path_str: &str) -> std::path::PathBuf {
    let file_path = Path::new(file_path_str);
    if file_path.is_absolute() {
        file_path.to_path_buf()
    } else {
        workspace.join(file_path)
    }
}

/// Path interner returning `Arc<PathBuf>` for shared-path deduplication.
///
/// Each unique `PathBuf` input is stored once as `Arc<PathBuf>` in an internal
/// `HashMap`; subsequent `intern` calls with the same path return a new
/// `Arc` clone pointing at the same heap allocation (pointer equality via
/// `Arc::ptr_eq`). This eliminates repeated `PathBuf` heap allocations when
/// many symbols share the same absolute file path (typical: a 200-symbol
/// `lib.rs` produces 200 entries with the same `PathBuf`).
///
/// # P-08 — why `PathBuf` as key (not `String`)
///
/// Using `PathBuf` directly as the HashMap key (rather than round-tripping
/// through `&str` via `to_string_lossy()`) preserves full byte fidelity on
/// non-UTF-8 paths (Linux allows arbitrary bytes in filesystem paths).
/// `PathBuf: Hash + Eq` is well-defined over the raw bytes. The earlier
/// `String`-keyed form would replace invalid UTF-8 bytes with `U+FFFD`,
/// breaking hover lookups for workspace paths containing non-UTF-8 chars.
///
/// # P-08 — why not `string-interner` crate
///
/// `string-interner` returns a `Symbol` (u32 index) that requires
/// `interner.resolve(symbol)` on every hover call — adds a lookup per hover.
/// `Arc<PathBuf>` derefs directly to `&Path` (zero-cost via `Arc::as_ref`),
/// matching `LspProvider::hover(&self, file: &Path, ...)` without trait
/// changes. The bottleneck is repeated heap allocation, not hash lookup.
///
/// # Lifetime
///
/// Local to `enhance_with_lsp` — the interner and its map are dropped when
/// the function returns. `Arc<PathBuf>` clones held by `entries` keep the
/// underlying `PathBuf` alive until `entries` is dropped.
///
/// # Allocation discipline (perf-review H-01)
///
/// `intern` first probes with `get(&path)` (zero allocation on hit). Only
/// on miss does it allocate the owned `PathBuf` for insertion. This avoids
/// the per-lookup `to_owned()` allocation churn of the `entry()` API form
/// (300k lookups × 80 B = 23.2 MB of wasted String allocations on hit path).
#[cfg(feature = "lsp")]
struct PathInterner {
    map: std::collections::HashMap<std::path::PathBuf, std::sync::Arc<std::path::PathBuf>>,
}

#[cfg(feature = "lsp")]
impl PathInterner {
    /// Creates an empty interner.
    #[must_use]
    fn new() -> Self {
        Self {
            map: std::collections::HashMap::new(),
        }
    }

    /// Returns an `Arc<PathBuf>` for `path`. If `path` was previously interned,
    /// returns a new `Arc` clone of the existing entry (pointer-equal via
    /// `Arc::ptr_eq`); otherwise inserts a new `Arc<PathBuf>` and returns it.
    ///
    /// Takes `PathBuf` by value so the caller can move an existing allocation
    /// in (typical: `resolve_abs_file_path` already returns `PathBuf`). On
    /// miss the moved-in `PathBuf` is wrapped in `Arc` and inserted; on hit
    /// the moved-in value is dropped (one extra drop, no extra allocation).
    ///
    /// # Empty path
    ///
    /// Interning `PathBuf::from("")` is allowed and returns a valid
    /// `Arc<PathBuf>` pointing at the empty path. The caller is responsible
    /// for not passing meaningless paths (hover on `""` will fail at LSP
    /// server side).
    fn intern(&mut self, path: std::path::PathBuf) -> std::sync::Arc<std::path::PathBuf> {
        // Hit path: clone the existing Arc (atomic refcount increment, ~5 ns).
        // Zero PathBuf allocation — the entry() form would allocate an owned
        // key for lookup even on hit (perf-review H-01: 290k hits × 80 B =
        // 23.2 MB of wasted String allocations).
        if let Some(arc) = self.map.get(&path) {
            return arc.clone();
        }
        // Miss path: wrap path in Arc (moves PathBuf, no clone), then clone
        // PathBuf back out for the map key. Net miss cost: 1 PathBuf clone
        // + 1 Arc allocation. Misses are rare (~10k vs ~290k hits) so the
        // extra clone is acceptable.
        let arc = std::sync::Arc::new(path);
        self.map.insert((*arc).clone(), arc.clone());
        arc
    }
}

/// LSP-driven `semantic_type` enhancement.
///
/// Spawns language servers (rust-analyzer, pyright-langserver, …) based on
/// file extensions found in the project, queries each symbol's hover info,
/// and writes the extracted type signature back to `semantic_type`.
/// Failures degrade gracefully to pure tree-sitter extraction.
///
/// # L7-6 memory-overflow fix — on-demand LSP startup
///
/// Pre-L7-6 this function unconditionally called `provider.start(workspace)`
/// for ALL 8 wired LSP clients (rs, py, c, cpp, go, ts, f90, java) before
/// querying any symbol. Each LSP server (rust-analyzer, pyright, clangd,
/// gopls, tsserver, fortls, jdtls) consumes 300 MB–2 GB of RSS, so a pure
/// Rust project needlessly spawned 7 unused servers — ~3–5 GB of wasted RSS
/// that compounded with the L7-5 buffer pool fix to cause OOM on 70 GB hosts.
///
/// L7-6 reorders the flow: query Function/Method rows FIRST, collect the
/// set of file extensions actually present, and only `start()` the LSP
/// providers whose extension appears in the result set. A pure Rust repo
/// now starts only `rust-analyzer`; a Python repo starts only `pyright`;
/// mixed repos start only the union. `shutdown()` is still called on all
/// 8 providers (the unused ones short-circuit to `Ok(())` per the
/// `LspProvider::shutdown` contract — "safe to call on a client that was
/// never successfully started").
///
/// Memory savings: ~3–5 GB on single-language repos (the common case).
#[cfg(feature = "lsp")]
#[allow(clippy::result_large_err)]
fn enhance_with_lsp(
    workspace: &Path,
    repo: &Repository,
    project: &str,
) -> Result<(), CodeNexusError> {
    use crate::lsp::LspError;

    let providers = build_lsp_providers();

    // L7-6: query Function/Method rows FIRST, then start only the LSP
    // providers whose extension appears in the result set. Pre-L7-6 started
    // all 8 providers up front, wasting ~3–5 GB on single-language repos.
    let queries = build_symbol_queries(project);
    let mut rows = Vec::new();
    for q in &queries {
        let r = repo.connection().query(q).map_err(|e| {
            CodeNexusError::Storage(crate::storage::StorageError::Query(e.to_string()))
        })?;
        rows.extend(r);
    }

    // Pre-extract (id, abs_file, line) tuples and collect the set of
    // extensions that actually need an LSP server. Rows that fail
    // `extract_lsp_row_fields` are counted as skipped (Rule 12: explicit
    // skip, not silent success — same behaviour as pre-L7-6).
    //
    // P-02: build `ext_map` once (HashMap<&str, &dyn LspProvider>) so the
    // hover loop's per-symbol `select_provider_for_ext` is O(1) instead of
    // the pre-P-02 linear scan over 8 providers.
    // P-10: build `provider_exts_set` once (HashSet<&str>) so `collect_active_extensions`
    // is also O(1) per entry instead of O(8).
    let provider_exts_set: std::collections::HashSet<&'static str> =
        providers.iter().map(|(e, _)| *e).collect();
    let ext_map: std::collections::HashMap<&'static str, &dyn LspProvider> = providers
        .iter()
        .map(|(ext, p)| (*ext, p.as_ref()))
        .collect();
    // P-08: interner deduplicates PathBuf heap allocations across entries.
    // A 200-symbol `lib.rs` produces 200 entries that previously each held
    // an independent ~80-byte PathBuf heap allocation; now they share one
    // Arc<PathBuf>. 10k files × 30 symbols = 300k entries → ~10k unique
    // Arc<PathBuf>, saving ~15-30 MB on large repos.
    let mut interner = PathInterner::new();
    let mut entries: Vec<(String, std::sync::Arc<std::path::PathBuf>, u32)> =
        Vec::with_capacity(rows.len());
    let mut skipped: u32 = 0;
    for row in &rows {
        let Some((id, file_path_str, start_line)) = extract_lsp_row_fields(row) else {
            skipped += 1;
            continue;
        };
        let abs_file = interner.intern(resolve_abs_file_path(workspace, &file_path_str));
        let line = u32::try_from(start_line).unwrap_or(0);
        entries.push((id, abs_file, line));
    }
    let active_exts = collect_active_extensions(&entries, &provider_exts_set);

    // P-04: parallel LSP startup via `std::thread::scope`. Pre-P-04 started
    // providers serially (rust-analyzer ~3-8 s, clangd ~1-3 s, gopls ~2-5 s),
    // so polyglot repos waited sum(per-provider) — up to ~20-40 s for 8-lang
    // repos. `thread::scope` spawns one thread per active provider; the main
    // thread joins all before continuing. No semaphore: at most 8 providers
    // (polyglot extreme), LSP startup is IO-bound (waiting on subprocess
    // pipes), so 8 threads on an 8-core machine is the sweet spot.
    let started_count = start_active_providers_parallel(&providers, &active_exts, workspace);
    eprintln!(
        "[info] LSP enhancement: started {started_count} of {} configured servers for {} symbols",
        providers.len(),
        entries.len()
    );

    // If no symbols need enhancement (empty repo / all rows failed to
    // extract), short-circuit — there is nothing to hover. Still call
    // shutdown() on all providers for symmetry.
    let mut enhanced: u32 = 0;
    // P-01: buffer hover results and flush in batches of LSP_HOVER_BATCH_SIZE
    // via a single Cypher `UNWIND ... SET` statement. Pre-P-01 issued one
    // `execute(&build_semantic_type_update(...))` per symbol — 100k symbols =
    // 100k synchronous round-trips to the embedded LadybugDB process, each
    // with ~1-3 ms commit overhead → 100-300 s of pure SQL waiting. The batch
    // path collapses 500 single-row UPDATEs into one statement, cutting
    // round-trips by 500×. On batch failure (e.g. LadybugDB rejects UNWIND),
    // `flush_semantic_type_batch` falls back to per-row execute so a single
    // bad statement does not poison the entire batch.
    let mut batch: Vec<(String, String)> = Vec::with_capacity(LSP_HOVER_BATCH_SIZE);
    if !entries.is_empty() {
        for (id, abs_file, line) in &entries {
            // P-08: abs_file is &Arc<PathBuf>; auto-deref-coerces to &Path
            // for Path::extension and LspProvider::hover.
            let ext = abs_file.extension().and_then(|e| e.to_str()).unwrap_or("");
            // M2/P-07: skip symbols whose extension has no wired LSP provider
            // (.md, .toml, .json). Pre-M2 fell back to providers[0]
            // (rust-analyzer) and wasted 300 MB-2 GB of RSS hovering files
            // LSP can't help with.
            let Some(client) = select_provider_for_ext(&ext_map, ext) else {
                skipped += 1;
                continue;
            };

            match client.hover(abs_file, *line, 0) {
                Ok(Some(hover)) => {
                    if let Some(text) = crate::lsp::extract_hover_text(&hover) {
                        batch.push((id.clone(), text));
                        if batch.len() >= LSP_HOVER_BATCH_SIZE {
                            flush_semantic_type_batch(
                                |q| repo.connection().execute(q),
                                &mut batch,
                                project,
                                &mut enhanced,
                                &mut skipped,
                            );
                        }
                    } else {
                        skipped += 1;
                    }
                }
                Ok(None) => {
                    skipped += 1;
                }
                Err(LspError::Timeout(_)) | Err(LspError::Communication(_)) => {
                    skipped += 1;
                }
                Err(LspError::ServerStart(_)) => {
                    skipped += 1;
                }
                Err(LspError::NotImplemented(_)) => {
                    // `hover` is implemented on all current clients; defensive
                    // skip for future opt-outs (C9 R-lsp-002 pattern).
                    skipped += 1;
                }
            }
        }
        // P-01: flush the tail — symbols that did not fill a complete batch.
        // `flush_semantic_type_batch` handles the empty case (returns 0 without
        // calling exec), so this call is safe even when entries.len() is an
        // exact multiple of LSP_HOVER_BATCH_SIZE.
        if !batch.is_empty() {
            flush_semantic_type_batch(
                |q| repo.connection().execute(q),
                &mut batch,
                project,
                &mut enhanced,
                &mut skipped,
            );
        }
    }

    eprintln!("[info] LSP enhancement: {enhanced} symbol(s) enhanced, {skipped} skipped");

    for (_ext, provider) in &providers {
        let _ = provider.shutdown();
    }
    Ok(())
}

/// Collects the set of provider extensions that appear in `entries` (L7-6).
///
/// Given the pre-extracted `(id, abs_file, line)` tuples from the
/// Function/Method query, returns the set of provider extensions whose LSP
/// server must be started.
///
/// # M2/P-07 — skip unknown extensions instead of falling back
///
/// Files whose extension does not match any wired provider (e.g. `.md`,
/// `.toml`, `.json`) are **skipped** — they do not activate any LSP server
/// and the hover loop later skips them via [`select_provider_for_ext`]
/// returning `None`. Pre-M2 fell back to `default_ext` (`"rs"`), which
/// needlessly started `rust-analyzer` for markdown/toml symbols, wasting
/// 300 MB-2 GB of RSS. Skipping is correct: LSP servers cannot hover
/// non-source files anyway, so the fallback hover result was always
/// `Ok(None)` (no semantic_type extracted) — the RSS cost was pure waste.
///
/// # P-10 — HashSet lookup replaces linear scan
///
/// `provider_exts` is now a `HashSet<&str>` instead of a `&[&str]`. The
/// per-entry `contains` is O(1) instead of O(8). For polyglot repos with
/// 100k+ symbols this avoids ~800k string comparisons during routing.
///
/// This is a pure function (no I/O, no LSP, no DB) so the L7-6 routing
/// decision can be unit-tested deterministically without spawning language
/// servers or opening a database.
///
/// # Arguments
///
/// * `entries` - Pre-extracted `(id, abs_file, line)` tuples from
///   `extract_lsp_row_fields`. Only the `abs_file` extension is read.
/// * `provider_exts` - HashSet of wired provider extensions (e.g.
///   `{"rs", "py", "c", "cpp", "go", "ts", "f90", "java"}`).
#[cfg(feature = "lsp")]
fn collect_active_extensions<'a>(
    entries: &[(String, std::sync::Arc<std::path::PathBuf>, u32)],
    provider_exts: &std::collections::HashSet<&'a str>,
) -> std::collections::HashSet<&'a str> {
    let mut active: std::collections::HashSet<&'a str> = std::collections::HashSet::new();
    for (_, abs_file, _) in entries {
        // P-08: abs_file is &Arc<PathBuf>; auto-deref-coerces to &Path.
        let ext = abs_file.extension().and_then(|e| e.to_str()).unwrap_or("");
        // M2/P-07: skip unknown extensions — do NOT fall back to default.
        if let Some(&provider_ext) = provider_exts.get(ext) {
            active.insert(provider_ext);
        }
    }
    active
}

/// Core index pipeline: indexer + DQ checks + CHECKPOINT + optional LSP.
///
/// Separated from the `#[forge]` function for reuse by `import.rs`
/// reindex logic.
#[cfg(any(feature = "cli", feature = "mcp", test))]
#[allow(clippy::result_large_err)]
// `lsp` parameter is only consumed under `feature = "lsp"`; suppress the
// unused-variable warning for builds without the lsp feature.
#[cfg_attr(not(feature = "lsp"), allow(unused_variables))]
pub(crate) fn index_core(
    kit: &AsyncKit<AsyncReady>,
    db_path: &Path,
    path: &str,
    name: &str,
    force: bool,
    lsp: bool,
    ram_first: bool,
) -> Result<IndexOutput, CodeNexusError> {
    let path_ref = Path::new(path);
    let indexer = kit.require::<IndexerModule>()?;
    let result = if ram_first {
        indexer.index_ram_first(path_ref, name, force)?
    } else {
        indexer.index(path_ref, name, force)?
    };

    // Open a FRESH Repository for DQ checks — the Kit's Storage connection
    // was opened at boot (before indexing) and holds a stale MVCC snapshot.
    let fresh_repo = Repository::open(db_path)?;
    let checker = QualityChecker::new(&fresh_repo);
    let dq_report = checker.run_all()?;
    if !dq_report.is_clean() {
        eprintln!("Data quality violations found:");
        for violation in &dq_report.violations {
            eprintln!(
                "  [{}] {} (project: {})",
                violation.rule,
                violation.message,
                violation.project.as_deref().unwrap_or("N/A")
            );
        }
    }

    // Flush the WAL after DQ checks.
    if let Err(err) = fresh_repo.connection().execute("CHECKPOINT;") {
        eprintln!("[warn] post-quality-check checkpoint failed: {err}");
    }

    // R-lsp-004: LSP-enhanced semantic_type extraction.
    #[cfg(feature = "lsp")]
    if lsp {
        if let Err(err) = enhance_with_lsp(path_ref, &fresh_repo, name) {
            eprintln!("[warn] LSP enhancement aborted: {err}");
        }
    }

    Ok(IndexOutput::from(result))
}

/// CLI wrapper — prints result to stdout as JSON.
//
// The Kit is stored in a static `OnceLock` (see `runtime::kit`), so it is
// never dropped — the stale boot-time connections never checkpoint over the
// indexer's writes (same effect as `std::mem::forget(kit)` in `main.rs`).
#[cfg(feature = "cli")]
#[forge(
    name = "index",
    version = "0.3.5",
    description = "Index a codebase into the knowledge graph.",
    cli = true
)]
async fn index(
    path: String,
    name: String,
    force: bool,
    lsp: bool,
    embed: bool,
    ram_first: bool,
) -> Result<(), ApiError> {
    if embed {
        eprintln!(
            "[warn] --embed flag is deprecated; embedding is controlled by \
             the `embed` cargo feature (rebuild with --features embed to enable)"
        );
    }

    let kit = kit().ok_or_else(kit_not_initialized)?;
    let storage_config = kit
        .config::<StorageConfig>()
        .map_err(|e| wrap_kit_error("Failed to resolve storage config", e))?;
    let db_path = storage_config.db_path.clone();

    let output = index_core(&kit, &db_path, &path, &name, force, lsp, ram_first)
        .map_err(|e| to_api_error(e, "index_error"))?;
    let json = serde_json::to_string(&output)
        .map_err(|e| to_api_error(CodeNexusError::from(e), "index_error"))?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "lsp")]
    mod path_interner_tests {
        use super::*;
        use std::path::PathBuf;
        use std::sync::Arc;

        #[test]
        fn intern_returns_same_arc_for_same_path() {
            let mut interner = PathInterner::new();
            let a = interner.intern(PathBuf::from("src/lib.rs"));
            let b = interner.intern(PathBuf::from("src/lib.rs"));
            assert!(
                Arc::ptr_eq(&a, &b),
                "same path must return pointer-equal Arc"
            );
        }

        #[test]
        fn intern_returns_different_arc_for_different_path() {
            let mut interner = PathInterner::new();
            let a = interner.intern(PathBuf::from("src/a.rs"));
            let b = interner.intern(PathBuf::from("src/b.rs"));
            assert!(!Arc::ptr_eq(&a, &b), "different paths must not share Arc");
        }

        #[test]
        fn intern_increments_strong_count() {
            let mut interner = PathInterner::new();
            let first = interner.intern(PathBuf::from("src/lib.rs"));
            let second = interner.intern(PathBuf::from("src/lib.rs"));
            let third = interner.intern(PathBuf::from("src/lib.rs"));
            // All three Arc clones point at the same heap allocation; the
            // strong count = map(1) + first(1) + second(1) + third(1) = 4.
            assert!(Arc::ptr_eq(&first, &second));
            assert!(Arc::ptr_eq(&second, &third));
            let count = Arc::strong_count(&third);
            assert!(
                count >= 3,
                "strong_count after 3 interns of same path should be >= 3 (map + first + second + third - 1 shared), got {count}"
            );
        }

        #[test]
        fn intern_handles_empty_path() {
            let mut interner = PathInterner::new();
            let arc = interner.intern(PathBuf::from(""));
            assert_eq!(arc.as_path(), std::path::Path::new(""));
        }

        #[test]
        fn intern_handles_unicode_path() {
            let mut interner = PathInterner::new();
            let arc = interner.intern(PathBuf::from("src/中文/文件.rs"));
            assert_eq!(arc.as_path(), std::path::Path::new("src/中文/文件.rs"));
            let arc2 = interner.intern(PathBuf::from("src/中文/文件.rs"));
            assert!(Arc::ptr_eq(&arc, &arc2));
        }

        #[test]
        fn path_interner_is_send_sync() {
            fn _assert_send_sync<T: Send + Sync>() {}
            _assert_send_sync::<PathInterner>();
        }
    }

    #[test]
    fn index_output_from_index_result_maps_all_fields() {
        let result = IndexResult {
            project_id: "p1".into(),
            files_indexed: 10,
            files_skipped: 2,
            nodes_created: 100,
            edges_created: 50,
            duration_ms: 5000,
        };
        let output = IndexOutput::from(result);
        assert_eq!(output.project_id, "p1");
        assert_eq!(output.files_indexed, 10);
        assert_eq!(output.files_skipped, 2);
        assert_eq!(output.nodes_created, 100);
        assert_eq!(output.edges_created, 50);
        assert_eq!(output.duration_ms, 5000);
    }

    #[test]
    fn index_output_from_handles_zero_values() {
        let result = IndexResult {
            project_id: "".into(),
            files_indexed: 0,
            files_skipped: 0,
            nodes_created: 0,
            edges_created: 0,
            duration_ms: 0,
        };
        let output = IndexOutput::from(result);
        assert_eq!(output.project_id, "");
        assert_eq!(output.files_indexed, 0);
        assert_eq!(output.nodes_created, 0);
        assert_eq!(output.edges_created, 0);
        assert_eq!(output.duration_ms, 0);
    }

    #[test]
    fn index_output_serializes_to_json() {
        let output = IndexOutput {
            project_id: "p1".into(),
            files_indexed: 10,
            files_skipped: 2,
            nodes_created: 100,
            edges_created: 50,
            duration_ms: 5000,
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"project_id\":\"p1\""));
        assert!(json.contains("\"files_indexed\":10"));
        assert!(json.contains("\"files_skipped\":2"));
        assert!(json.contains("\"nodes_created\":100"));
        assert!(json.contains("\"edges_created\":50"));
        assert!(json.contains("\"duration_ms\":5000"));
    }

    #[test]
    fn index_output_from_preserves_large_values() {
        let result = IndexResult {
            project_id: "uuid-v7-12345".into(),
            files_indexed: usize::MAX,
            files_skipped: usize::MAX,
            nodes_created: usize::MAX,
            edges_created: usize::MAX,
            duration_ms: u64::MAX,
        };
        let output = IndexOutput::from(result);
        assert_eq!(output.files_indexed, usize::MAX);
        assert_eq!(output.duration_ms, u64::MAX);
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn index_core_indexes_rust_project() {
        use crate::kit::{build_kit, KitBootstrapConfig};
        use std::fs;
        use tempfile::TempDir;

        let src_dir = TempDir::new().unwrap();
        let src_path = src_dir.path();
        fs::write(
            src_path.join("main.rs"),
            "fn main() { println!(\"hello\"); }\n",
        )
        .unwrap();

        let db_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join("index_testdb");
        let config = KitBootstrapConfig::new(db_path.clone());
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit");

        let output = index_core(
            &kit,
            &db_path,
            src_path.to_str().unwrap(),
            "test_project",
            false,
            false,
            false,
        )
        .expect("index should succeed");

        assert!(!output.project_id.is_empty());
        assert!(output.files_indexed >= 1);
        assert!(output.nodes_created > 0);
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn index_core_force_reindexes_unchanged_files() {
        use crate::kit::{build_kit, KitBootstrapConfig};
        use std::fs;
        use tempfile::TempDir;

        let src_dir = TempDir::new().unwrap();
        let src_path = src_dir.path();
        fs::write(
            src_path.join("lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        )
        .unwrap();

        let db_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join("index_force_testdb");
        let config = KitBootstrapConfig::new(db_path.clone());
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit");

        // First index
        let output1 = index_core(
            &kit,
            &db_path,
            src_path.to_str().unwrap(),
            "force_project",
            false,
            false,
            false,
        )
        .expect("first index should succeed");
        assert!(output1.files_indexed >= 1);

        // Second index with force=true should reindex even though files are unchanged
        let output2 = index_core(
            &kit,
            &db_path,
            src_path.to_str().unwrap(),
            "force_project",
            true,
            false,
            false,
        )
        .expect("forced reindex should succeed");
        assert!(output2.files_indexed >= 1);
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn index_core_handles_empty_directory() {
        use crate::kit::{build_kit, KitBootstrapConfig};
        use tempfile::TempDir;

        let src_dir = TempDir::new().unwrap();
        let src_path = src_dir.path();

        let db_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join("index_empty_testdb");
        let config = KitBootstrapConfig::new(db_path.clone());
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit");

        let output = index_core(
            &kit,
            &db_path,
            src_path.to_str().unwrap(),
            "empty_project",
            false,
            false,
            false,
        )
        .expect("index should succeed on empty dir");

        assert_eq!(output.files_indexed, 0);
        assert_eq!(output.nodes_created, 0);
    }

    // Covers line 188: ram_first=true branch in index_core.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn index_core_with_ram_first_indexes_rust_project() {
        use crate::kit::{build_kit, KitBootstrapConfig};
        use std::fs;
        use tempfile::TempDir;

        let src_dir = TempDir::new().unwrap();
        let src_path = src_dir.path();
        fs::write(
            src_path.join("lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        )
        .unwrap();

        let db_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join("index_ram_first_testdb");
        let config = KitBootstrapConfig::new(db_path.clone());
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit");

        let output = index_core(
            &kit,
            &db_path,
            src_path.to_str().unwrap(),
            "ram_first_project",
            false,
            false,
            true,
        )
        .expect("ram_first index should succeed");

        assert!(!output.project_id.is_empty());
        assert!(output.files_indexed >= 1);
        assert!(output.nodes_created > 0);
    }

    // Covers line 188: ram_first=true with force=true reindex path.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn index_core_with_ram_first_and_force_reindexes() {
        use crate::kit::{build_kit, KitBootstrapConfig};
        use std::fs;
        use tempfile::TempDir;

        let src_dir = TempDir::new().unwrap();
        let src_path = src_dir.path();
        fs::write(
            src_path.join("main.rs"),
            "fn main() { println!(\"hello\"); }\n",
        )
        .unwrap();

        let db_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join("index_ram_force_testdb");
        let config = KitBootstrapConfig::new(db_path.clone());
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit");

        // First index with ram_first=true.
        let output1 = index_core(
            &kit,
            &db_path,
            src_path.to_str().unwrap(),
            "ram_force_project",
            false,
            false,
            true,
        )
        .expect("first ram_first index should succeed");
        assert!(output1.files_indexed >= 1);

        // Second index with ram_first=true and force=true.
        let output2 = index_core(
            &kit,
            &db_path,
            src_path.to_str().unwrap(),
            "ram_force_project",
            true,
            false,
            true,
        )
        .expect("forced ram_first reindex should succeed");
        assert!(output2.files_indexed >= 1);
    }

    // --- Pure function tests (extracted from enhance_with_lsp) ---

    #[cfg(feature = "lsp")]
    #[test]
    fn build_lsp_providers_returns_8_providers() {
        let providers = build_lsp_providers();
        assert_eq!(providers.len(), 8);
        let exts: Vec<&str> = providers.iter().map(|(e, _)| *e).collect();
        assert!(exts.contains(&"rs"));
        assert!(exts.contains(&"py"));
        assert!(exts.contains(&"go"));
        assert!(exts.contains(&"java"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn build_symbol_queries_contains_project_name() {
        let queries = build_symbol_queries("demo");
        assert!(queries[0].contains("demo"));
        assert!(queries[1].contains("demo"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn build_symbol_queries_escapes_special_chars() {
        let queries = build_symbol_queries("demo' OR '1'='1");
        assert!(!queries[0].contains("' OR '1'='1"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn select_provider_for_ext_returns_some_for_rs() {
        // P-02: select_provider_for_ext now does a HashMap lookup and
        // returns Option<&dyn LspProvider>. .rs must map to a provider.
        let providers = build_lsp_providers();
        let ext_map: std::collections::HashMap<&'static str, &dyn LspProvider> = providers
            .iter()
            .map(|(ext, p)| (*ext, p.as_ref()))
            .collect();
        let provider = select_provider_for_ext(&ext_map, "rs").expect("rs must map to a provider");
        let _ = provider.shutdown();
        // Drop providers after shutdown to release the Box<dyn LspProvider>.
        drop(providers);
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn select_provider_for_ext_returns_none_for_unknown() {
        // M2/P-07: unknown extensions (.md, .toml, .json) no longer fall
        // back to providers[0]. select_provider_for_ext returns None so the
        // hover loop skips the symbol — pre-M2 wasted 300 MB-2 GB of RSS
        // starting rust-analyzer for non-source files.
        let providers = build_lsp_providers();
        let ext_map: std::collections::HashMap<&'static str, &dyn LspProvider> = providers
            .iter()
            .map(|(ext, p)| (*ext, p.as_ref()))
            .collect();
        assert!(
            select_provider_for_ext(&ext_map, "md").is_none(),
            "md must NOT map to any provider"
        );
        assert!(
            select_provider_for_ext(&ext_map, "toml").is_none(),
            "toml must NOT map to any provider"
        );
        assert!(
            select_provider_for_ext(&ext_map, "unknown").is_none(),
            "unknown ext must NOT map to any provider"
        );
        // Shutdown all providers (no-op for never-started clients).
        for (_, p) in &providers {
            let _ = p.shutdown();
        }
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn build_semantic_type_update_escapes_id() {
        let update = build_semantic_type_update("id'with'quotes", "demo", "type_text");
        assert!(update.contains(r"id\'with\'quotes"));
    }

    // --- extract_lsp_row_fields tests ---

    #[cfg(feature = "lsp")]
    #[test]
    fn extract_lsp_row_fields_returns_all_fields_when_valid() {
        let row = vec![
            serde_json::Value::String("sym_id".into()),
            serde_json::Value::String("/src/main.rs".into()),
            serde_json::Value::Number(serde_json::Number::from(42u64)),
        ];
        let (id, file_path, start_line) =
            extract_lsp_row_fields(&row).expect("valid row should extract");
        assert_eq!(id, "sym_id");
        assert_eq!(file_path, "/src/main.rs");
        assert_eq!(start_line, 42);
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn extract_lsp_row_fields_returns_none_when_id_missing() {
        let row = vec![
            serde_json::Value::Null,
            serde_json::Value::String("/src/main.rs".into()),
            serde_json::Value::Number(serde_json::Number::from(1u64)),
        ];
        assert!(extract_lsp_row_fields(&row).is_none());
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn extract_lsp_row_fields_returns_none_when_file_path_missing() {
        let row = vec![
            serde_json::Value::String("sym_id".into()),
            serde_json::Value::Null,
            serde_json::Value::Number(serde_json::Number::from(1u64)),
        ];
        assert!(extract_lsp_row_fields(&row).is_none());
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn extract_lsp_row_fields_returns_none_when_start_line_missing() {
        let row = vec![
            serde_json::Value::String("sym_id".into()),
            serde_json::Value::String("/src/main.rs".into()),
            serde_json::Value::Null,
        ];
        assert!(extract_lsp_row_fields(&row).is_none());
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn extract_lsp_row_fields_returns_none_for_empty_row() {
        let row: Vec<serde_json::Value> = vec![];
        assert!(extract_lsp_row_fields(&row).is_none());
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn extract_lsp_row_fields_returns_none_when_id_not_string() {
        let row = vec![
            serde_json::Value::Number(serde_json::Number::from(123u64)),
            serde_json::Value::String("/src/main.rs".into()),
            serde_json::Value::Number(serde_json::Number::from(1u64)),
        ];
        assert!(extract_lsp_row_fields(&row).is_none());
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn extract_lsp_row_fields_returns_none_when_start_line_not_number() {
        let row = vec![
            serde_json::Value::String("sym_id".into()),
            serde_json::Value::String("/src/main.rs".into()),
            serde_json::Value::String("not_a_number".into()),
        ];
        assert!(extract_lsp_row_fields(&row).is_none());
    }

    // --- resolve_abs_file_path tests ---

    #[cfg(feature = "lsp")]
    #[test]
    fn resolve_abs_file_path_returns_absolute_unchanged() {
        let workspace = std::path::Path::new("/workspace");
        let abs = resolve_abs_file_path(workspace, "/src/main.rs");
        assert_eq!(abs, std::path::PathBuf::from("/src/main.rs"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn resolve_abs_file_path_joins_relative_with_workspace() {
        let workspace = std::path::Path::new("/workspace");
        let abs = resolve_abs_file_path(workspace, "src/main.rs");
        assert_eq!(abs, std::path::PathBuf::from("/workspace/src/main.rs"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn resolve_abs_file_path_handles_empty_string() {
        let workspace = std::path::Path::new("/workspace");
        let abs = resolve_abs_file_path(workspace, "");
        assert_eq!(abs, std::path::PathBuf::from("/workspace"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn resolve_abs_file_path_handles_relative_with_dots() {
        let workspace = std::path::Path::new("/workspace");
        let abs = resolve_abs_file_path(workspace, "./src/main.rs");
        assert_eq!(abs, std::path::PathBuf::from("/workspace/./src/main.rs"));
    }

    // --- collect_active_extensions (L7-6) tests ---
    //
    // L7-6 memory-overflow fix: only start LSP servers for extensions that
    // actually appear in the indexed symbols. These tests verify the routing
    // decision without spawning any LSP server or opening a database.

    #[cfg(feature = "lsp")]
    #[test]
    fn collect_active_extensions_empty_entries_returns_empty_set() {
        let provider_exts: std::collections::HashSet<&str> =
            ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"]
                .into_iter()
                .collect();
        let active = collect_active_extensions(&[], &provider_exts);
        assert!(
            active.is_empty(),
            "empty entries must produce empty active set"
        );
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn collect_active_extensions_pure_rust_returns_only_rs() {
        let provider_exts: std::collections::HashSet<&str> =
            ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"]
                .into_iter()
                .collect();
        // P-08: entries now hold Arc<PathBuf> instead of PathBuf.
        let entries = vec![
            (
                "id1".to_string(),
                std::sync::Arc::new(std::path::PathBuf::from("/src/main.rs")),
                1,
            ),
            (
                "id2".to_string(),
                std::sync::Arc::new(std::path::PathBuf::from("/src/lib.rs")),
                10,
            ),
            (
                "id3".to_string(),
                std::sync::Arc::new(std::path::PathBuf::from("/src/mod.rs")),
                20,
            ),
        ];
        let active = collect_active_extensions(&entries, &provider_exts);
        assert_eq!(
            active.len(),
            1,
            "pure Rust repo must activate only rust-analyzer"
        );
        assert!(active.contains("rs"));
        // Critical L7-6 invariant: must NOT start unused LSP servers.
        assert!(!active.contains("py"));
        assert!(!active.contains("c"));
        assert!(!active.contains("cpp"));
        assert!(!active.contains("go"));
        assert!(!active.contains("ts"));
        assert!(!active.contains("f90"));
        assert!(!active.contains("java"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn collect_active_extensions_pure_python_returns_only_py() {
        let provider_exts: std::collections::HashSet<&str> =
            ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"]
                .into_iter()
                .collect();
        let entries = vec![
            (
                "id1".to_string(),
                std::sync::Arc::new(std::path::PathBuf::from("/src/main.py")),
                1,
            ),
            (
                "id2".to_string(),
                std::sync::Arc::new(std::path::PathBuf::from("/src/utils.py")),
                5,
            ),
        ];
        let active = collect_active_extensions(&entries, &provider_exts);
        assert_eq!(
            active.len(),
            1,
            "pure Python repo must activate only pyright"
        );
        assert!(active.contains("py"));
        assert!(!active.contains("rs"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn collect_active_extensions_mixed_rs_py_returns_both() {
        let provider_exts: std::collections::HashSet<&str> =
            ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"]
                .into_iter()
                .collect();
        let entries = vec![
            (
                "id1".to_string(),
                std::sync::Arc::new(std::path::PathBuf::from("/src/main.rs")),
                1,
            ),
            (
                "id2".to_string(),
                std::sync::Arc::new(std::path::PathBuf::from("/src/script.py")),
                5,
            ),
        ];
        let active = collect_active_extensions(&entries, &provider_exts);
        assert_eq!(
            active.len(),
            2,
            "mixed repo must activate exactly the union"
        );
        assert!(active.contains("rs"));
        assert!(active.contains("py"));
        assert!(!active.contains("java"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn collect_active_extensions_unknown_extension_skipped() {
        // M2/P-07: unknown extensions (.md, .toml, .json) are SKIPPED — they
        // do NOT activate any LSP server. Pre-M2 fell back to default ("rs"),
        // needlessly starting rust-analyzer for markdown/toml files (waste of
        // 300 MB-2 GB RSS). Skipping is correct because LSP servers cannot
        // hover non-source files anyway.
        let provider_exts: std::collections::HashSet<&str> =
            ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"]
                .into_iter()
                .collect();
        let entries = vec![
            (
                "id1".to_string(),
                std::sync::Arc::new(std::path::PathBuf::from("/README.md")),
                1,
            ),
            (
                "id2".to_string(),
                std::sync::Arc::new(std::path::PathBuf::from("/Cargo.toml")),
                1,
            ),
        ];
        let active = collect_active_extensions(&entries, &provider_exts);
        assert!(
            active.is_empty(),
            "unknown extensions (.md, .toml) must NOT activate any LSP server; got: {active:?}"
        );
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn collect_active_extensions_cpp_and_c_both_active() {
        // ClangdClient is wired for both "c" and "cpp"; a project with both
        // .c and .cpp files activates both extension keys (which both map
        // to the same ClangdClient, but the routing decision is per-ext).
        let provider_exts: std::collections::HashSet<&str> =
            ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"]
                .into_iter()
                .collect();
        let entries = vec![
            (
                "id1".to_string(),
                std::sync::Arc::new(std::path::PathBuf::from("/src/main.c")),
                1,
            ),
            (
                "id2".to_string(),
                std::sync::Arc::new(std::path::PathBuf::from("/src/util.cpp")),
                5,
            ),
        ];
        let active = collect_active_extensions(&entries, &provider_exts);
        assert_eq!(active.len(), 2);
        assert!(active.contains("c"));
        assert!(active.contains("cpp"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn collect_active_extensions_dedupes_repeated_extensions() {
        // 1000 .rs files should activate exactly one extension ("rs"), not
        // 1000 entries — the HashSet deduplicates.
        let provider_exts: std::collections::HashSet<&str> =
            ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"]
                .into_iter()
                .collect();
        let entries: Vec<(String, std::sync::Arc<std::path::PathBuf>, u32)> = (0..1000)
            .map(|i| {
                (
                    format!("id{i}"),
                    std::sync::Arc::new(std::path::PathBuf::from("/src/mod.rs")),
                    i,
                )
            })
            .collect();
        let active = collect_active_extensions(&entries, &provider_exts);
        assert_eq!(active.len(), 1);
        assert!(active.contains("rs"));
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn collect_active_extensions_all_eight_exts_active_for_polyglot_repo() {
        // A polyglot repo with all 8 wired extensions activates all 8
        // providers (worst case — no savings vs pre-L7-6, but correctness
        // preserved).
        let provider_exts: std::collections::HashSet<&str> =
            ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"]
                .into_iter()
                .collect();
        let entries = vec![
            (
                "id1".to_string(),
                std::sync::Arc::new(std::path::PathBuf::from("/a.rs")),
                1,
            ),
            (
                "id2".to_string(),
                std::sync::Arc::new(std::path::PathBuf::from("/b.py")),
                1,
            ),
            (
                "id3".to_string(),
                std::sync::Arc::new(std::path::PathBuf::from("/c.c")),
                1,
            ),
            (
                "id4".to_string(),
                std::sync::Arc::new(std::path::PathBuf::from("/d.cpp")),
                1,
            ),
            (
                "id5".to_string(),
                std::sync::Arc::new(std::path::PathBuf::from("/e.go")),
                1,
            ),
            (
                "id6".to_string(),
                std::sync::Arc::new(std::path::PathBuf::from("/f.ts")),
                1,
            ),
            (
                "id7".to_string(),
                std::sync::Arc::new(std::path::PathBuf::from("/g.f90")),
                1,
            ),
            (
                "id8".to_string(),
                std::sync::Arc::new(std::path::PathBuf::from("/h.java")),
                1,
            ),
        ];
        let active = collect_active_extensions(&entries, &provider_exts);
        assert_eq!(
            active.len(),
            8,
            "polyglot repo must activate all 8 providers"
        );
    }

    // --- #[forge] index wrapper tests ---

    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn index_wrapper_fails_when_kit_not_initialized() {
        use crate::service::runtime::reset_kit_for_testing;

        reset_kit_for_testing();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(index(
            "/nonexistent/path".to_string(),
            "test_project".to_string(),
            false,
            false,
            false,
            false,
        ));
        assert!(result.is_err(), "wrapper should fail without kit");
        reset_kit_for_testing();
    }

    #[serial_test::serial(kit_init)]
    #[cfg(all(feature = "cli", feature = "lang-rust"))]
    #[test]
    fn index_wrapper_succeeds_via_init_kit() {
        use crate::kit::{build_kit, KitBootstrapConfig};
        use crate::service::runtime::{init_kit, reset_kit_for_testing};
        use std::fs;
        use tempfile::TempDir;

        reset_kit_for_testing();

        let src_dir = TempDir::new().unwrap();
        fs::write(src_dir.path().join("main.rs"), "fn main() {}\n").unwrap();

        let db_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join("wrapper_testdb");
        let config = KitBootstrapConfig::new(db_path.clone());
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit");
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(index(
            src_dir.path().to_str().unwrap().to_string(),
            "wrapper_test".to_string(),
            true,
            false,
            false,
            false,
        ));
        assert!(result.is_ok(), "wrapper should succeed: {:?}", result.err());

        reset_kit_for_testing();
    }

    // Covers the `embed=true` deprecation warning branch (lines 296-301).
    #[serial_test::serial(kit_init)]
    #[cfg(all(feature = "cli", feature = "lang-rust"))]
    #[test]
    fn index_wrapper_with_embed_true_emits_deprecation_warning() {
        use crate::kit::{build_kit, KitBootstrapConfig};
        use crate::service::runtime::{init_kit, reset_kit_for_testing};
        use std::fs;
        use tempfile::TempDir;

        reset_kit_for_testing();

        let src_dir = TempDir::new().unwrap();
        fs::write(src_dir.path().join("main.rs"), "fn main() {}\n").unwrap();

        let db_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join("embed_warn_testdb");
        let config = KitBootstrapConfig::new(db_path.clone());
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit");
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(index(
            src_dir.path().to_str().unwrap().to_string(),
            "embed_warn_test".to_string(),
            true,
            false,
            true,
            false,
        ));
        assert!(
            result.is_ok(),
            "wrapper should succeed even with embed=true: {:?}",
            result.err()
        );

        reset_kit_for_testing();
    }

    // Covers the DQ violation reporting branch in index_core (lines 248-258).
    // Pre-populates the DB with a File node that has an empty hash (DQ-006
    // violation), then calls index_core. The DQ check should detect the
    // violation and trigger the `if !dq_report.is_clean()` branch.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn index_core_reports_dq_violations_when_present() {
        use crate::kit::{build_kit, KitBootstrapConfig};
        use crate::storage::Repository;
        use std::fs;
        use tempfile::TempDir;

        let src_dir = TempDir::new().unwrap();
        let src_path = src_dir.path();
        fs::write(src_path.join("main.rs"), "fn main() {}\n").unwrap();

        let db_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join("dq_violation_testdb");
        let config = KitBootstrapConfig::new(db_path.clone());
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit");

        // Pre-populate the DB with a DQ-006 violation: a File node with empty
        // hash. Use a separate project name to avoid conflicts with the
        // indexer. The data is committed via a fresh Repository connection
        // (LadybugDB auto-commits each execute), so the fresh_repo opened
        // inside index_core will see it.
        let repo = Repository::open(&db_path).expect("open repo for injection");
        repo.connection()
            .execute(
                "CREATE (:File {id: 'bad_file', project: 'ghost_proj', name: 'bad.rs', \
                 filePath: '/bad.rs', language: 'rust', hash: '', lineCount: 0});",
            )
            .expect("insert bad file");

        // Call index_core — the DQ check should detect the empty hash and
        // trigger the `if !dq_report.is_clean()` branch (lines 248-258).
        // DQ violations are reported via eprintln! but don't fail indexing.
        let output = index_core(
            &kit,
            &db_path,
            src_path.to_str().unwrap(),
            "test_project",
            false,
            false,
            false,
        )
        .expect("index should succeed even with DQ violations");

        assert!(output.files_indexed >= 1);
    }

    // Covers the `kit.config::<StorageConfig>().map_err(...)` error path
    // (lines 304-306) in the index wrapper. Creates a kit WITHOUT registering
    // StorageModule or setting StorageConfig, then calls the index wrapper.
    #[serial_test::serial(kit_init)]
    #[cfg(feature = "cli")]
    #[test]
    fn index_wrapper_fails_when_storage_config_not_registered() {
        use crate::kit::AsyncKit;
        use crate::service::runtime::{init_kit, reset_kit_for_testing};
        use sdforge::prelude::ApiError;

        reset_kit_for_testing();

        // Build an empty kit (no modules registered, no configs set).
        // build() succeeds on an empty dependency graph.
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(AsyncKit::new().build())
            .expect("build empty kit");
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(index(
            "/nonexistent/path".to_string(),
            "test_project".to_string(),
            false,
            false,
            false,
            false,
        ));
        assert!(result.is_err(), "wrapper should fail without StorageConfig");
        // The error should be an Internal ApiError with a message mentioning
        // storage config (from wrap_kit_error).
        match result.unwrap_err() {
            ApiError::Internal { message, .. } => {
                assert!(
                    message.contains("storage config"),
                    "error should mention storage config: {message}"
                );
            }
            other => panic!("expected Internal error, got {other:?}"),
        }

        reset_kit_for_testing();
    }

    // --- P-01: build_batch_semantic_type_update + flush_semantic_type_batch ---

    #[cfg(feature = "lsp")]
    mod batch_update_tests {
        use super::*;
        use crate::storage::StorageError;

        #[test]
        fn build_batch_empty_returns_empty_string() {
            // P-01: empty batch must short-circuit — calling UNWIND [] would
            // be a no-op but also wastes a round-trip. Return "" so the
            // caller can skip the execute entirely.
            let batch: Vec<(String, String)> = Vec::new();
            let stmt = build_batch_semantic_type_update(&batch, "proj");
            assert!(stmt.is_empty(), "empty batch must return empty string");
        }

        #[test]
        fn build_batch_single_entry_generates_valid_unwind_statement() {
            // P-01: a single-entry batch must produce a syntactically valid
            // Cypher UNWIND ... MATCH ... SET statement.
            let batch = vec![(String::from("sym1"), String::from("fn foo() -> i32"))];
            let stmt = build_batch_semantic_type_update(&batch, "my-project");
            assert!(
                stmt.starts_with("UNWIND ["),
                "must start with UNWIND [: got {stmt}"
            );
            assert!(
                stmt.contains("{id: 'sym1', sem: 'fn foo() -> i32'}"),
                "must contain the entry map: got {stmt}"
            );
            assert!(
                stmt.contains("] AS row"),
                "must close list and alias as row: got {stmt}"
            );
            assert!(
                stmt.contains("MATCH (n {id: row.id, project: 'my-project'})"),
                "must MATCH by row.id + project: got {stmt}"
            );
            assert!(
                stmt.contains("SET n.semantic_type = row.sem"),
                "must SET semantic_type from row.sem: got {stmt}"
            );
            assert!(
                stmt.ends_with(';'),
                "statement must end with semicolon: got {stmt}"
            );
        }

        #[test]
        fn build_batch_500_entries_contains_500_id_sem_maps() {
            // P-01: a 500-entry batch must emit exactly 500 `{id: '...', sem: '...'}`
            // maps. This is the contract that makes the UNWIND batch worth the
            // round-trip — 500 single-row UPDATEs collapse into one statement.
            //
            // Match on `{id: '` (with trailing single quote) to distinguish
            // entry maps from the `MATCH (n {id: row.id, ...})` clause which
            // uses `{id: row.` (no quote after colon).
            let batch: Vec<(String, String)> = (0..500)
                .map(|i| (format!("sym_{i}"), format!("type_{i}")))
                .collect();
            let stmt = build_batch_semantic_type_update(&batch, "proj");
            let count = stmt.matches("{id: '").count();
            assert_eq!(count, 500, "expected 500 entry maps, got {count}");
        }

        #[test]
        fn build_batch_escapes_single_quotes_in_id() {
            // P-01: id values containing single quotes must be escaped via
            // escape_cypher_string (single quote -> "\'"). Without escaping
            // the Cypher string literal would terminate early and the
            // statement would be a syntax error.
            let batch = vec![(String::from("a'b"), String::from("type"))];
            let stmt = build_batch_semantic_type_update(&batch, "proj");
            assert!(
                stmt.contains("{id: 'a\\'b', sem: 'type'}"),
                "id single quote must be escaped: got {stmt}"
            );
        }

        #[test]
        fn build_batch_escapes_single_quotes_in_sem() {
            // P-01: sem values containing single quotes must also be escaped.
            let batch = vec![(String::from("sym"), String::from("fn it's() -> ()"))];
            let stmt = build_batch_semantic_type_update(&batch, "proj");
            assert!(
                stmt.contains("{id: 'sym', sem: 'fn it\\'s() -> ()'}"),
                "sem single quote must be escaped: got {stmt}"
            );
        }

        #[test]
        fn build_batch_escapes_single_quotes_in_project() {
            // P-01: project containing single quotes must be escaped in the
            // MATCH clause's `project: '...'` literal.
            let batch = vec![(String::from("sym"), String::from("type"))];
            let stmt = build_batch_semantic_type_update(&batch, "proj'ect");
            assert!(
                stmt.contains("project: 'proj\\'ect'"),
                "project single quote must be escaped: got {stmt}"
            );
        }

        #[test]
        fn build_batch_escapes_backslashes_in_id_and_sem() {
            // P-01: backslashes must be doubled (escape_cypher_string order:
            // backslash first to avoid double-escaping later steps).
            let batch = vec![(String::from("a\\b"), String::from("c\\d"))];
            let stmt = build_batch_semantic_type_update(&batch, "proj");
            assert!(
                stmt.contains("{id: 'a\\\\b', sem: 'c\\\\d'}"),
                "backslashes must be doubled: got {stmt}"
            );
        }

        #[test]
        fn flush_batch_empty_does_not_call_exec_and_returns_zero() {
            // P-01: empty batch must short-circuit — exec must NOT be called
            // (verified via a counter closure), and the return value must be 0.
            let mut batch: Vec<(String, String)> = Vec::new();
            let mut enhanced: u32 = 0;
            let mut skipped: u32 = 0;
            let call_count = std::cell::Cell::new(0u32);
            let exec = |_q: &str| -> crate::storage::Result<()> {
                call_count.set(call_count.get() + 1);
                Ok(())
            };
            flush_semantic_type_batch(exec, &mut batch, "proj", &mut enhanced, &mut skipped);
            assert_eq!(
                call_count.get(),
                0,
                "exec must not be called on empty batch"
            );
            assert_eq!(enhanced, 0);
            assert_eq!(skipped, 0);
            assert!(batch.is_empty(), "batch must remain empty");
        }

        #[test]
        fn flush_batch_500_entries_calls_exec_once_when_batch_succeeds() {
            // P-01: when the batch UNWIND UPDATE succeeds, exec is called
            // exactly once (the batch path), enhanced += 500, skipped += 0,
            // batch is cleared, return value = 500.
            let mut batch: Vec<(String, String)> = (0..500)
                .map(|i| (format!("sym_{i}"), format!("type_{i}")))
                .collect();
            let mut enhanced: u32 = 0;
            let mut skipped: u32 = 0;
            let call_count = std::cell::Cell::new(0u32);
            let exec = |_q: &str| -> crate::storage::Result<()> {
                call_count.set(call_count.get() + 1);
                Ok(())
            };
            flush_semantic_type_batch(exec, &mut batch, "proj", &mut enhanced, &mut skipped);
            assert_eq!(
                call_count.get(),
                1,
                "exec must be called exactly once on batch success"
            );
            assert_eq!(enhanced, 500, "all 500 entries counted as enhanced");
            assert_eq!(skipped, 0);
            assert!(batch.is_empty(), "batch must be cleared after flush");
        }

        #[test]
        fn flush_batch_falls_back_to_per_row_when_batch_exec_fails() {
            // P-01: when the batch UNWIND UPDATE fails, flush must fall back
            // to per-row `build_semantic_type_update` + exec. Each successful
            // per-row exec increments enhanced; each failure increments skipped.
            // The batch is cleared regardless.
            let mut batch: Vec<(String, String)> = (0..10)
                .map(|i| (format!("sym_{i}"), format!("type_{i}")))
                .collect();
            let mut enhanced: u32 = 0;
            let mut skipped: u32 = 0;
            // Track which queries we've seen. The first call (batch) fails;
            // subsequent calls (per-row) succeed for even-indexed symbols and
            // fail for odd-indexed (to verify both enhanced and skipped paths).
            let call_count = std::cell::Cell::new(0u32);
            let exec = |_q: &str| -> crate::storage::Result<()> {
                let n = call_count.get();
                call_count.set(n + 1);
                if n == 0 {
                    // First call = batch UNWIND — fail to trigger fallback.
                    Err(StorageError::Query(String::from(
                        "UNWIND syntax not supported",
                    )))
                } else {
                    // Per-row calls (n=1..10). sym_0..sym_9 — even succeeds, odd fails.
                    // n=1 -> sym_0 (even) -> Ok
                    // n=2 -> sym_1 (odd)  -> Err
                    // ...
                    let row_idx = (n - 1) as usize;
                    if row_idx.is_multiple_of(2) {
                        Ok(())
                    } else {
                        Err(StorageError::Query(String::from("per-row fail")))
                    }
                }
            };
            flush_semantic_type_batch(exec, &mut batch, "proj", &mut enhanced, &mut skipped);
            // 1 batch call + 10 per-row calls = 11 total.
            assert_eq!(
                call_count.get(),
                11,
                "must call batch once then per-row 10 times"
            );
            // sym_0, sym_2, sym_4, sym_6, sym_8 succeed = 5 enhanced.
            assert_eq!(enhanced, 5, "5 even-indexed per-row calls succeed");
            // sym_1, sym_3, sym_5, sym_7, sym_9 fail = 5 skipped.
            assert_eq!(skipped, 5, "5 odd-indexed per-row calls fail");
            assert!(batch.is_empty(), "batch must be cleared after flush");
        }

        #[test]
        fn flush_batch_full_failure_increments_skipped_only() {
            // P-01: when both batch exec and all per-row execs fail, enhanced
            // stays at its initial value and skipped increments by batch.len().
            // The batch is still cleared regardless of failure.
            let mut batch: Vec<(String, String)> = (0..3)
                .map(|i| (format!("sym_{i}"), format!("type_{i}")))
                .collect();
            let mut enhanced: u32 = 99;
            let mut skipped: u32 = 99;
            let exec = |_q: &str| -> crate::storage::Result<()> {
                Err(StorageError::Query(String::from("always fail")))
            };
            flush_semantic_type_batch(exec, &mut batch, "proj", &mut enhanced, &mut skipped);
            // Batch fails (1 call), then 3 per-row calls all fail.
            // enhanced unchanged (0 new), skipped += 3.
            assert_eq!(enhanced, 99, "no per-row success");
            assert_eq!(skipped, 102, "3 per-row failures increment skipped from 99");
            assert!(batch.is_empty(), "batch cleared");
        }
    }

    // --- P-04: parallel LSP startup via std::thread::scope ---

    #[cfg(feature = "lsp")]
    mod parallel_start_tests {
        use super::*;
        use crate::lsp::{LspError, LspProvider};
        use lsp_types::{Hover, Location};
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        /// Mock LSP provider for P-04 parallel-start tests.
        ///
        /// Records `start()` calls in an `AtomicU32` so the count is visible
        /// across threads without locking. `start_delay_ms` simulates the
        /// subprocess startup latency of a real LSP server (rust-analyzer
        /// ~3-8 s, clangd ~1-3 s). `should_panic` triggers a panic inside
        /// `start()` to verify that `catch_unwind` isolates buggy providers.
        struct MockLspProvider {
            start_count: Arc<AtomicU32>,
            start_delay_ms: u64,
            should_panic: bool,
            should_fail: bool,
        }

        impl MockLspProvider {
            fn new(start_delay_ms: u64) -> Self {
                Self {
                    start_count: Arc::new(AtomicU32::new(0)),
                    start_delay_ms,
                    should_panic: false,
                    should_fail: false,
                }
            }

            fn with_panic(mut self) -> Self {
                self.should_panic = true;
                self
            }

            fn with_failure(mut self) -> Self {
                self.should_fail = true;
                self
            }
        }

        impl LspProvider for MockLspProvider {
            fn start(&self, _workspace: &Path) -> Result<(), LspError> {
                self.start_count.fetch_add(1, Ordering::SeqCst);
                if self.should_panic {
                    panic!("MockLspProvider: simulated panic");
                }
                if self.start_delay_ms > 0 {
                    std::thread::sleep(Duration::from_millis(self.start_delay_ms));
                }
                if self.should_fail {
                    return Err(LspError::ServerStart(String::from(
                        "MockLspProvider: simulated failure",
                    )));
                }
                Ok(())
            }

            fn definition(
                &self,
                _file: &Path,
                _line: u32,
                _col: u32,
            ) -> Result<Option<Location>, LspError> {
                Ok(None)
            }

            fn type_definition(
                &self,
                _file: &Path,
                _line: u32,
                _col: u32,
            ) -> Result<Option<Location>, LspError> {
                Ok(None)
            }

            fn hover(
                &self,
                _file: &Path,
                _line: u32,
                _col: u32,
            ) -> Result<Option<Hover>, LspError> {
                Ok(None)
            }

            fn shutdown(&self) -> Result<(), LspError> {
                Ok(())
            }
        }

        /// Provider vector type used by [`build_mock_providers`] (P-04 tests).
        ///
        /// Factored into a type alias to satisfy `clippy::type_complexity` —
        /// the raw `(Vec<(&'static str, Box<dyn LspProvider>)>, Vec<Arc<AtomicU32>>)`
        /// form exceeds the default complexity threshold.
        type MockProvidersResult = (
            Vec<(&'static str, Box<dyn LspProvider>)>,
            Vec<Arc<AtomicU32>>,
        );

        /// Builds mock providers with per-provider delays. Returns the
        /// provider vec (production signature) plus the start_count Arcs
        /// so tests can verify call counts after the parallel start.
        fn build_mock_providers(delays_ms: &[u64]) -> MockProvidersResult {
            // Static extension strings — required because the production
            // signature uses `&'static str`. Tests use a fixed set.
            const EXTS: [&str; 8] = ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"];
            let mut providers: Vec<(&'static str, Box<dyn LspProvider>)> = Vec::new();
            let mut counts: Vec<Arc<AtomicU32>> = Vec::new();
            for (i, &delay) in delays_ms.iter().enumerate() {
                let ext = EXTS[i];
                let mock = MockLspProvider::new(delay);
                counts.push(mock.start_count.clone());
                providers.push((ext, Box::new(mock) as Box<dyn LspProvider>));
            }
            // Pad to 8 if fewer were provided (unused extensions).
            while providers.len() < 8 {
                let ext = EXTS[providers.len()];
                let mock = MockLspProvider::new(0);
                providers.push((ext, Box::new(mock) as Box<dyn LspProvider>));
            }
            (providers, counts)
        }

        #[test]
        fn parallel_start_completes_faster_than_serial() {
            // P-04 core invariant: 8 providers each sleeping 50 ms must
            // complete in < 200 ms (serial would be 400 ms). The 200 ms
            // bound gives 4× headroom over the theoretical 50 ms parallel
            // minimum, accommodating thread spawn/sleep jitter.
            let (providers, _counts) = build_mock_providers(&[50, 50, 50, 50, 50, 50, 50, 50]);
            // Build active_exts = all 8 extensions.
            let active_exts: std::collections::HashSet<&'static str> =
                providers.iter().map(|(ext, _)| *ext).collect();

            let workspace = PathBuf::from("/tmp/mock-workspace");
            let start = Instant::now();
            let started = start_active_providers_parallel(&providers, &active_exts, &workspace);
            let elapsed = start.elapsed();

            assert_eq!(started, 8, "all 8 providers should start successfully");
            assert!(
                elapsed < Duration::from_millis(200),
                "parallel start must complete in < 200 ms, got {elapsed:?}"
            );
        }

        #[test]
        fn parallel_start_calls_start_on_all_active_providers() {
            // P-04: every active provider's `start()` must be called exactly
            // once. Verifies that the scope spawns one thread per active
            // provider and does not skip any.
            let delays = [10, 10, 10, 10, 10, 10, 10, 10];
            const EXTS: [&str; 8] = ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"];
            let mut providers: Vec<(&'static str, Box<dyn LspProvider>)> = Vec::new();
            let mut counts: Vec<Arc<AtomicU32>> = Vec::new();
            for (i, &delay) in delays.iter().enumerate() {
                let mock = MockLspProvider::new(delay);
                counts.push(mock.start_count.clone());
                providers.push((EXTS[i], Box::new(mock) as Box<dyn LspProvider>));
            }
            let active_exts: std::collections::HashSet<&'static str> =
                EXTS.iter().copied().collect();

            let workspace = PathBuf::from("/tmp/mock-workspace");
            let started = start_active_providers_parallel(&providers, &active_exts, &workspace);

            assert_eq!(started, 8);
            for (i, count) in counts.iter().enumerate() {
                assert_eq!(
                    count.load(Ordering::SeqCst),
                    1,
                    "provider {} start_count must be 1",
                    EXTS[i]
                );
            }
        }

        #[test]
        fn parallel_start_skips_inactive_providers() {
            // P-04: providers whose extension is NOT in `active_exts` must
            // not be started. Verifies the filter step before scope spawn.
            const EXTS: [&str; 8] = ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"];
            let mut providers: Vec<(&'static str, Box<dyn LspProvider>)> = Vec::new();
            let mut counts: Vec<Arc<AtomicU32>> = Vec::new();
            for ext in EXTS.iter() {
                let mock = MockLspProvider::new(5);
                counts.push(mock.start_count.clone());
                providers.push((*ext, Box::new(mock) as Box<dyn LspProvider>));
            }
            // Only "rs" is active.
            let active_exts: std::collections::HashSet<&'static str> =
                std::collections::HashSet::from(["rs"]);

            let workspace = PathBuf::from("/tmp/mock-workspace");
            let started = start_active_providers_parallel(&providers, &active_exts, &workspace);

            assert_eq!(started, 1, "only 1 provider should be started");
            assert_eq!(
                counts[0].load(Ordering::SeqCst),
                1,
                "rs provider called once"
            );
            for i in 1..8 {
                assert_eq!(
                    counts[i].load(Ordering::SeqCst),
                    0,
                    "provider {} must NOT be started (not in active_exts)",
                    EXTS[i]
                );
            }
        }

        #[test]
        fn parallel_start_panic_does_not_block_other_providers() {
            // P-04: a panicking provider must be caught by `catch_unwind` and
            // converted to a `LspError::ServerStart` message. The remaining
            // providers must still complete successfully.
            const EXTS: [&str; 8] = ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"];
            let mut providers: Vec<(&'static str, Box<dyn LspProvider>)> = Vec::new();
            let mut counts: Vec<Arc<AtomicU32>> = Vec::new();
            for (i, ext) in EXTS.iter().enumerate() {
                let mut mock = MockLspProvider::new(10);
                if i == 2 {
                    // "c" provider panics.
                    mock = mock.with_panic();
                }
                counts.push(mock.start_count.clone());
                providers.push((*ext, Box::new(mock) as Box<dyn LspProvider>));
            }
            let active_exts: std::collections::HashSet<&'static str> =
                EXTS.iter().copied().collect();

            let workspace = PathBuf::from("/tmp/mock-workspace");
            let started = start_active_providers_parallel(&providers, &active_exts, &workspace);

            // 7 of 8 started (the panicking "c" provider counts as not started).
            assert_eq!(
                started, 7,
                "7 of 8 providers should start; panicking one is degraded"
            );
            // The panicking provider's start_count must still be 1 (the fetch_add
            // happens before the panic).
            assert_eq!(
                counts[2].load(Ordering::SeqCst),
                1,
                "panicking provider was called"
            );
            // All non-panicking providers called once.
            for (i, count) in counts.iter().enumerate() {
                if i == 2 {
                    continue;
                }
                assert_eq!(
                    count.load(Ordering::SeqCst),
                    1,
                    "provider {} called once",
                    EXTS[i]
                );
            }
        }

        #[test]
        fn parallel_start_failure_does_not_block_other_providers() {
            // P-04: a provider returning `Err(LspError::ServerStart)` must be
            // logged and skipped; the remaining providers must still complete.
            const EXTS: [&str; 8] = ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"];
            let mut providers: Vec<(&'static str, Box<dyn LspProvider>)> = Vec::new();
            let mut counts: Vec<Arc<AtomicU32>> = Vec::new();
            for (i, ext) in EXTS.iter().enumerate() {
                let mut mock = MockLspProvider::new(5);
                if i == 1 {
                    // "py" provider returns failure.
                    mock = mock.with_failure();
                }
                counts.push(mock.start_count.clone());
                providers.push((*ext, Box::new(mock) as Box<dyn LspProvider>));
            }
            let active_exts: std::collections::HashSet<&'static str> =
                EXTS.iter().copied().collect();

            let workspace = PathBuf::from("/tmp/mock-workspace");
            let started = start_active_providers_parallel(&providers, &active_exts, &workspace);

            // 7 of 8 started.
            assert_eq!(
                started, 7,
                "7 of 8 providers should start; failing one is degraded"
            );
            for (i, count) in counts.iter().enumerate() {
                assert_eq!(
                    count.load(Ordering::SeqCst),
                    1,
                    "provider {} called once",
                    EXTS[i]
                );
            }
        }

        #[test]
        fn parallel_start_empty_active_exts_returns_zero() {
            // P-04: empty active_exts must short-circuit without spawning
            // any threads (early return after filter).
            const EXTS: [&str; 8] = ["rs", "py", "c", "cpp", "go", "ts", "f90", "java"];
            let mut providers: Vec<(&'static str, Box<dyn LspProvider>)> = Vec::new();
            let mut counts: Vec<Arc<AtomicU32>> = Vec::new();
            for ext in EXTS.iter() {
                let mock = MockLspProvider::new(5);
                counts.push(mock.start_count.clone());
                providers.push((*ext, Box::new(mock) as Box<dyn LspProvider>));
            }
            let active_exts: std::collections::HashSet<&'static str> =
                std::collections::HashSet::new();

            let workspace = PathBuf::from("/tmp/mock-workspace");
            let started = start_active_providers_parallel(&providers, &active_exts, &workspace);

            assert_eq!(
                started, 0,
                "no providers should be started on empty active_exts"
            );
            for (i, count) in counts.iter().enumerate() {
                assert_eq!(
                    count.load(Ordering::SeqCst),
                    0,
                    "provider {} must not be called",
                    EXTS[i]
                );
            }
        }
    }
}
