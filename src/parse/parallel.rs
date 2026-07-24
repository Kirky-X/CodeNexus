// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Parallel file parsing using rayon (ADR-010).
//!
//! Provides file-level parallelism for the extraction phase: each file is
//! parsed independently by a rayon worker thread, and thread-local
//! [`ParserPool`](super::parser_pool::ParserPool) instances cache parsers per
//! thread (ADR-010). Because the parsing phase does not create cross-file
//! edges (those are created in the resolve phase), results can be merged
//! lock-free — each file produces its own [`ExtractResult`].
//!
//! Parse failures are logged and skipped: a single bad file never stops the
//! whole batch. Failures are collected in [`ParallelParseResult::errors`]
//! alongside the successful [`ParallelParseResult::results`].

use rayon::prelude::*;
use tracing::{debug, warn};

use crate::discover::FileInfo;
use crate::parse::error::ParseError;
use crate::parse::extractor::{extract_file, extract_from_source, ExtractResult};

// ---------------------------------------------------------------------------
// ParallelParseResult
// ---------------------------------------------------------------------------

/// The result of parsing a batch of files in parallel.
///
/// Successful extractions are collected in [`results`](Self::results), while
/// per-file failures (e.g. I/O errors, unknown language) are collected in
/// [`errors`](Self::errors) without aborting the batch.
#[derive(Debug)]
pub struct ParallelParseResult {
    /// Successfully extracted results, one per file that parsed without error.
    pub results: Vec<ExtractResult>,
    /// Per-file failures as `(relative_path, error_message)` pairs.
    pub errors: Vec<(String, String)>,
    /// Number of files that were parsed successfully.
    pub files_parsed: usize,
    /// Number of files that failed to parse.
    pub files_failed: usize,
}

// ---------------------------------------------------------------------------
// parallel_parse
// ---------------------------------------------------------------------------

/// Parses multiple files in parallel using rayon (ADR-010).
///
/// Each file is parsed independently (file-level parallelism). Parse failures
/// are collected rather than propagated, so a single bad file does not stop
/// the batch. Results are merged lock-free: each file produces its own
/// [`ExtractResult`] and the parsing phase creates no cross-file edges (those
/// are created in the resolve phase).
///
/// # Arguments
///
/// * `files` - The discovered files to parse.
/// * `project` - The project name (used for node `project` field, DDD §2.3).
///
/// # Returns
///
/// A [`ParallelParseResult`] containing the successful extractions, the
/// per-file failures, and the success/failure counts.
#[must_use]
pub fn parallel_parse(files: &[FileInfo], project: &str) -> ParallelParseResult {
    // File-level parallelism (ADR-010): each file is parsed independently by
    // a rayon worker thread. Thread-local ParserPool instances cache parsers
    // per thread (see extract_file -> get_extractor -> ParserFactory).
    //
    // Lock-free merge: the parsing phase creates no cross-file edges (those
    // are created in the resolve phase), so each file produces its own
    // ExtractResult that can be collected without synchronization.
    //
    // Parse failures are collected, not propagated: a single bad file never
    // stops the batch.
    let results: Vec<Result<ExtractResult, (String, String)>> = files
        .par_iter()
        .map(|file| {
            let lang = file
                .language
                .ok_or_else(|| (file.relative_path.clone(), "unknown language".to_string()))?;
            let result = extract_file(&file.path, lang, project)
                .map_err(|e| (file.relative_path.clone(), e.to_string()))?;
            debug!(
                event = "file_parsed",
                path = %file.relative_path,
                language = %lang,
                nodes = result.nodes.len(),
                "file parsed successfully"
            );
            Ok(result)
        })
        .collect();

    // Separate successes and failures lock-free (post-collection).
    let mut ok_results = Vec::with_capacity(files.len());
    let mut errors = Vec::new();
    for result in results {
        match result {
            Ok(r) => ok_results.push(r),
            Err((path, msg)) => errors.push((path, msg)),
        }
    }

    let files_parsed = ok_results.len();
    let files_failed = errors.len();

    ParallelParseResult {
        results: ok_results,
        errors,
        files_parsed,
        files_failed,
    }
}

// ---------------------------------------------------------------------------
// parse_single
// ---------------------------------------------------------------------------

/// Parses a single file (for sequential use or testing).
///
/// # Errors
///
/// Returns [`ParseError::UnsupportedLanguage`] if the file's language is
/// `None`, or a [`ParseError::Io`] / [`ParseError::ParseFailed`] from
/// [`extract_file`] if the file cannot be read or parsed.
pub fn parse_single(file: &FileInfo, project: &str) -> Result<ExtractResult, ParseError> {
    let lang = file
        .language
        .ok_or_else(|| ParseError::UnsupportedLanguage("unknown".to_string()))?;
    extract_file(&file.path, lang, project)
}

// ---------------------------------------------------------------------------
// parallel_parse_ram_first (H15)
// ---------------------------------------------------------------------------

/// LZ4-compressed source buffer keyed by absolute file path.
///
/// Built by [`IndexFacade::index_ram_first`] before the DAG runs: each
/// changed/added file is read from disk, LZ4-compressed into a `Vec<u8>`, and
/// stored in this map. The parse phase decompresses on demand (one file per
/// rayon worker), bounding peak memory to `compressed_total + N * max_file`
/// where N is the worker count.
///
/// [`IndexFacade::index_ram_first`]: crate::index::IndexFacade::index_ram_first
pub type RamFirstSources = std::collections::HashMap<std::path::PathBuf, Vec<u8>>;

/// Parses multiple files in parallel from LZ4-compressed in-memory buffers
/// (H15/D9 RAM-first indexing).
///
/// For each file in `files`, if its `path` is present in `compressed`, the
/// LZ4-compressed bytes are decompressed into a `String` and parsed via
/// [`extract_from_source`] (no disk read). Files not in the map fall back to
/// the normal [`extract_file`] disk-read path — this keeps the function robust
/// if the map was built from a slightly different file set (e.g. a file was
/// created between the pre-scan and the DAG scan).
///
/// # L2 memory-bounding refactor
///
/// Pre-L2 used `files.par_iter().map(parse).collect::<Vec<_>>()` which
/// forces rayon to materialize every `ExtractResult` in an intermediate
/// `Vec` before the caller can drain it. For large repos this drives peak
/// memory up to roughly `sum(all_extract_results) + N * max_source`
/// (where N is the rayon worker count), because all results live in the
/// collected `Vec` simultaneously.
///
/// The streaming path uses:
/// * `par_chunks(CHUNK_SIZE)` — each worker processes a batch of files
///   sequentially, reducing rayon scheduling overhead vs. per-file dispatch
///   while keeping the per-worker memory bound unchanged (one decompressed
///   `String` at a time).
/// * `mpsc::sync_channel(CHANNEL_BOUND)` — workers send each result through
///   a bounded channel; when the main thread is slow to drain, workers block
///   on `send`, capping in-flight `ExtractResult` count at `CHANNEL_BOUND`
///   instead of `files.len()`.
///
/// The main thread drains the channel and pushes successes/failures into the
/// result `Vec`s in arrival order. The public API and `ParallelParseResult`
/// shape are unchanged — this is a pure internal memory-bounding refactor.
///
/// # Arguments
///
/// * `files` - The discovered files to parse (from `ScanPhase`).
/// * `compressed` - LZ4-compressed source bytes keyed by absolute path.
/// * `project` - The project name (used for node `project` field, DDD §2.3).
///
/// # Returns
///
/// A [`ParallelParseResult`] — same shape as [`parallel_parse`].
pub fn parallel_parse_ram_first(
    files: &[FileInfo],
    compressed: &RamFirstSources,
    project: &str,
) -> ParallelParseResult {
    // L2: chunked dispatch + bounded-channel backpressure.
    const CHUNK_SIZE: usize = 8;
    const CHANNEL_BOUND: usize = 4;

    if files.is_empty() {
        return ParallelParseResult {
            results: Vec::new(),
            errors: Vec::new(),
            files_parsed: 0,
            files_failed: 0,
        };
    }

    let (tx, rx) =
        std::sync::mpsc::sync_channel::<Result<ExtractResult, (String, String)>>(CHANNEL_BOUND);

    // Worker closure: parse one file and return Ok(ExtractResult) or
    // Err((relative_path, message)) matching the pre-L2 error shape.
    // Captures `compressed` and `project` by reference — both are Sync so
    // the closure is Send + Sync and safe to invoke from rayon workers.
    let parse_one = |file: &FileInfo| -> Result<ExtractResult, (String, String)> {
        let lang = file
            .language
            .ok_or_else(|| (file.relative_path.clone(), "unknown language".to_string()))?;
        if let Some(comp_bytes) = compressed.get(&file.path) {
            // RAM-first path: LZ4-decompress into String, parse, drop.
            let raw = lz4_flex::decompress_size_prepended(comp_bytes).map_err(|e| {
                (
                    file.relative_path.clone(),
                    format!("LZ4 decompress failed: {e}"),
                )
            })?;
            let source = String::from_utf8(raw).map_err(|e| {
                (
                    file.relative_path.clone(),
                    format!("UTF-8 decode failed: {e}"),
                )
            })?;
            let result =
                extract_from_source(&file.path.display().to_string(), &source, lang, project)
                    .map_err(|e| (file.relative_path.clone(), e.to_string()))?;
            // `source` dropped here (decompressed bytes released).
            debug!(
                event = "file_parsed_ram_first",
                path = %file.relative_path,
                language = %lang,
                nodes = result.nodes.len(),
                "file parsed from RAM-first buffer"
            );
            Ok(result)
        } else {
            // Fallback: file not in compressed map — read from disk.
            warn!(
                path = %file.relative_path,
                "RAM-first: file not in compressed map, falling back to disk read"
            );
            extract_file(&file.path, lang, project)
                .map_err(|e| (file.relative_path.clone(), e.to_string()))
        }
    };

    // CRITICAL: par_chunks + bounded channel must run concurrently with the
    // rx drain loop. `for_each_with` is blocking, so if we called it inline
    // the workers would block on `send` (channel full) while the main
    // thread blocked on `for_each_with` returning — a classic deadlock.
    //
    // `std::thread::scope` spawns a producer thread that drives the rayon
    // parallelism; the main thread drains `rx` concurrently. When all
    // workers finish, the producer thread drops the original `tx` (and the
    // `for_each_with` clones drop as workers exit), the channel closes, and
    // the rx iterator terminates.
    //
    // Tracing subscriber propagation: `tracing`'s subscriber is thread-local
    // and neither `std::thread::spawn` nor rayon workers inherit the parent
    // thread's current subscriber. We snapshot the current `Dispatch` and
    // re-install it inside both the producer thread and each rayon worker's
    // body via `tracing::dispatcher::with_default`, so `debug!`/`warn!`
    // events emitted by `parse_one` reach test capture subscribers (and
    // production subscribers alike).
    let dispatcher = tracing::dispatcher::get_default(tracing::Dispatch::clone);

    let mut ok_results = Vec::with_capacity(files.len());
    let mut errors = Vec::new();
    std::thread::scope(|s| {
        s.spawn(|| {
            tracing::dispatcher::with_default(&dispatcher, || {
                files
                    .par_chunks(CHUNK_SIZE)
                    .for_each_with(tx, |worker_tx, chunk| {
                        tracing::dispatcher::with_default(&dispatcher, || {
                            for file in chunk {
                                let result = parse_one(file);
                                if worker_tx.send(result).is_err() {
                                    // Receiver dropped — abort this chunk.
                                    return;
                                }
                            }
                        });
                    });
            });
            // `tx` dropped here when the producer thread exits. Worker
            // clones drop as workers finish, so the channel closes once all
            // in-flight chunks complete.
        });

        // Main thread: drain the channel in arrival order, splitting Ok/Err
        // into the result Vecs. The loop exits when all senders are gone.
        for result in rx {
            match result {
                Ok(r) => ok_results.push(r),
                Err((path, msg)) => errors.push((path, msg)),
            }
        }
    });

    let files_parsed = ok_results.len();
    let files_failed = errors.len();

    ParallelParseResult {
        results: ok_results,
        errors,
        files_parsed,
        files_failed,
    }
}

#[cfg(all(
    test,
    feature = "lang-c",
    feature = "lang-python",
    feature = "lang-rust"
))]
mod tests {
    use super::*;
    use crate::model::Language;
    use crate::test_log_capture::drain_to_string;
    use crossbeam_channel::unbounded;
    use inklog::domain::core::LoggerSubscriber;
    use inklog::{LogRecord, Metrics};
    use rayon::ThreadPoolBuilder;
    use std::cell::RefCell;
    use std::sync::Arc;
    use tracing_subscriber::filter::LevelFilter;
    use tracing_subscriber::prelude::*;

    /// Builds a `FileInfo` for a file written to `dir/name` with `content`
    /// and the given language.
    fn make_file(
        dir: &std::path::Path,
        name: &str,
        content: &str,
        language: Option<Language>,
    ) -> FileInfo {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        FileInfo {
            path,
            relative_path: name.to_string(),
            language,
            size: content.len() as u64,
        }
    }

    // -----------------------------------------------------------------------
    // 1. Parallel parse of multiple Rust files
    // -----------------------------------------------------------------------

    #[test]
    fn test_parallel_parse_multiple_rust_files() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![
            make_file(dir.path(), "a.rs", "fn foo() {}", Some(Language::Rust)),
            make_file(dir.path(), "b.rs", "fn bar() {}", Some(Language::Rust)),
        ];

        let result = parallel_parse(&files, "testproject");
        assert_eq!(result.files_parsed, 2);
        assert_eq!(result.files_failed, 0);
        assert_eq!(result.results.len(), 2);
        assert!(result.errors.is_empty());
    }

    // -----------------------------------------------------------------------
    // 2. Parallel parse of mixed languages (C, Rust, Python)
    // -----------------------------------------------------------------------

    #[test]
    fn test_parallel_parse_mixed_languages() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![
            make_file(dir.path(), "a.rs", "fn foo() {}", Some(Language::Rust)),
            make_file(
                dir.path(),
                "b.c",
                "int foo(void) { return 0; }",
                Some(Language::C),
            ),
            make_file(
                dir.path(),
                "c.py",
                "def foo(): pass",
                Some(Language::Python),
            ),
        ];

        let result = parallel_parse(&files, "proj");
        assert_eq!(result.files_parsed, 3);
        assert_eq!(result.files_failed, 0);
        assert_eq!(result.results.len(), 3);

        // Each result should carry its own language.
        let langs: Vec<Language> = result.results.iter().map(|r| r.language).collect();
        assert!(langs.contains(&Language::Rust));
        assert!(langs.contains(&Language::C));
        assert!(langs.contains(&Language::Python));
    }

    // -----------------------------------------------------------------------
    // 3. Parse failure handling: one bad file, others succeed
    // -----------------------------------------------------------------------

    #[test]
    fn test_parallel_parse_failure_handling() {
        let dir = tempfile::tempdir().unwrap();
        let good = make_file(dir.path(), "a.rs", "fn foo() {}", Some(Language::Rust));
        // A FileInfo pointing at a file that does not exist on disk triggers
        // an Io error inside extract_file.
        let bad = FileInfo {
            path: dir.path().join("does_not_exist.rs"),
            relative_path: "does_not_exist.rs".to_string(),
            language: Some(Language::Rust),
            size: 0,
        };
        let files = vec![good, bad];

        let result = parallel_parse(&files, "proj");
        assert_eq!(result.files_parsed, 1, "the good file should parse");
        assert_eq!(result.files_failed, 1, "the bad file should fail");
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].0, "does_not_exist.rs");
        assert!(
            !result.errors[0].1.is_empty(),
            "error message should be non-empty"
        );
    }

    // -----------------------------------------------------------------------
    // 4. Empty file list
    // -----------------------------------------------------------------------

    #[test]
    fn test_parallel_parse_empty_list() {
        let files: Vec<FileInfo> = vec![];
        let result = parallel_parse(&files, "proj");
        assert_eq!(result.files_parsed, 0);
        assert_eq!(result.files_failed, 0);
        assert!(result.results.is_empty());
        assert!(result.errors.is_empty());
    }

    // -----------------------------------------------------------------------
    // 5. Files with unknown language (None) are treated as errors
    // -----------------------------------------------------------------------

    #[test]
    fn test_parallel_parse_unknown_language_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![make_file(dir.path(), "a.rs", "fn foo() {}", None)];

        let result = parallel_parse(&files, "proj");
        assert_eq!(result.files_parsed, 0);
        assert_eq!(result.files_failed, 1);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].0, "a.rs");
        assert!(
            result.errors[0].1.contains("unknown language"),
            "error should mention unknown language: {}",
            result.errors[0].1
        );
    }

    // -----------------------------------------------------------------------
    // 6. parallel_parse returns correct counts
    // -----------------------------------------------------------------------

    #[test]
    fn test_parallel_parse_correct_counts() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![
            make_file(dir.path(), "a.rs", "fn a() {}", Some(Language::Rust)),
            make_file(dir.path(), "b.rs", "fn b() {}", Some(Language::Rust)),
            make_file(dir.path(), "c.rs", "fn c() {}", Some(Language::Rust)),
            // Two failures: a missing file and an unknown language.
            FileInfo {
                path: dir.path().join("missing.rs"),
                relative_path: "missing.rs".to_string(),
                language: Some(Language::Rust),
                size: 0,
            },
            FileInfo {
                path: dir.path().join("unknown.rs"),
                relative_path: "unknown.rs".to_string(),
                language: None,
                size: 0,
            },
        ];

        let result = parallel_parse(&files, "proj");
        assert_eq!(result.files_parsed, 3);
        assert_eq!(result.files_failed, 2);
        assert_eq!(result.results.len(), 3);
        assert_eq!(result.errors.len(), 2);
        // files_parsed + files_failed should equal the input length.
        assert_eq!(result.files_parsed + result.files_failed, files.len());
    }

    // -----------------------------------------------------------------------
    // 7. parse_single works for a single file
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_single_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let file = make_file(dir.path(), "a.rs", "fn foo() {}", Some(Language::Rust));

        let result = parse_single(&file, "proj");
        assert!(
            result.is_ok(),
            "parse_single should succeed: {:?}",
            result.err()
        );
        let result = result.unwrap();
        assert_eq!(result.language, Language::Rust);
        assert!(!result.nodes.is_empty(), "should extract nodes");
        assert!(
            result.nodes.iter().any(|n| n.name == "foo"),
            "should extract the foo function: {:?}",
            result.nodes.iter().map(|n| &n.name).collect::<Vec<_>>()
        );
    }

    // -----------------------------------------------------------------------
    // 8. parse_single returns error for unknown language
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_single_unknown_language_errors() {
        let dir = tempfile::tempdir().unwrap();
        let file = make_file(dir.path(), "a.rs", "fn foo() {}", None);

        let result = parse_single(&file, "proj");
        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::UnsupportedLanguage(_) => { /* expected */ }
            other => panic!("expected UnsupportedLanguage, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // 9. Thread safety: parallel_parse works with many files (10+)
    // -----------------------------------------------------------------------

    #[test]
    fn test_parallel_parse_thread_safety_many_files() {
        let dir = tempfile::tempdir().unwrap();
        let count = 15;
        let files: Vec<FileInfo> = (0..count)
            .map(|i| {
                make_file(
                    dir.path(),
                    &format!("file_{i}.rs"),
                    &format!("fn func_{i}() {{}}"),
                    Some(Language::Rust),
                )
            })
            .collect();

        let result = parallel_parse(&files, "proj");
        assert_eq!(result.files_parsed, count);
        assert_eq!(result.files_failed, 0);
        assert_eq!(result.results.len(), count);
        assert!(result.errors.is_empty());

        // Every function name should appear among the extracted nodes.
        let mut all_names: Vec<String> = result
            .results
            .iter()
            .flat_map(|r| r.nodes.iter().map(|n| n.name.clone()))
            .collect();
        all_names.sort();
        for i in 0..count {
            assert!(
                all_names.contains(&format!("func_{i}")),
                "missing func_{i} in extracted nodes"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 10. Results contain correct nodes/edges from extraction
    // -----------------------------------------------------------------------

    #[test]
    fn test_parallel_parse_results_contain_nodes_and_edges() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![make_file(
            dir.path(),
            "a.rs",
            "fn foo() {}\nfn bar() {}\n",
            Some(Language::Rust),
        )];

        let result = parallel_parse(&files, "proj");
        assert_eq!(result.files_parsed, 1);
        assert_eq!(result.results.len(), 1);

        let extract = &result.results[0];
        // Nodes: both foo and bar should be extracted as Function nodes.
        let names: Vec<&str> = extract.nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"foo"), "should extract foo: {names:?}");
        assert!(names.contains(&"bar"), "should extract bar: {names:?}");
        // Edges: the extractor creates Contains/Defines edges for definitions.
        assert!(
            !extract.edges.is_empty(),
            "should extract edges for definitions"
        );
        // Each result should carry the project name on its edges.
        for edge in &extract.edges {
            assert_eq!(edge.project, "proj", "edge project should match");
        }
    }

    // -----------------------------------------------------------------------
    // Bonus: parallel_parse preserves order independence (results count)
    // -----------------------------------------------------------------------

    #[test]
    fn test_parallel_parse_preserves_total_count() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![
            make_file(dir.path(), "a.rs", "fn a() {}", Some(Language::Rust)),
            make_file(dir.path(), "b.py", "def b(): pass", Some(Language::Python)),
            make_file(
                dir.path(),
                "c.c",
                "int c(void) { return 0; }",
                Some(Language::C),
            ),
        ];

        let result = parallel_parse(&files, "proj");
        // Every input file produced exactly one outcome (success or failure).
        assert_eq!(result.results.len() + result.errors.len(), files.len());
        assert_eq!(result.files_parsed + result.files_failed, files.len());
    }

    // -----------------------------------------------------------------------
    // LOG-002: file_parsed event emission
    // -----------------------------------------------------------------------

    // Thread-local storage for the tracing `DefaultGuard` on rayon worker
    // threads. Each worker thread sets its own subscriber via
    // `tracing::subscriber::set_default` and stores the guard here so it
    // stays alive for the thread's lifetime.
    thread_local! {
        static TRACING_GUARD: RefCell<Option<tracing::subscriber::DefaultGuard>> =
            const { RefCell::new(None) };
    }

    /// Runs `f` inside a scoped tracing subscriber (DEBUG and above) that
    /// captures all event output into a string, returning that string.
    ///
    /// Because `parallel_parse` uses rayon worker threads that do NOT inherit
    /// the current thread's tracing subscriber, this helper builds a custom
    /// rayon thread pool whose `start_handler` installs an inklog
    /// `LoggerSubscriber` (sharing the same console channel) on each worker
    /// thread. The test function `f` is then run via `pool.install()` so that
    /// `par_iter` calls inside `f` use worker threads that have the subscriber
    /// set.
    fn capture_tracing_debug<R: Send>(f: impl FnOnce() -> R + Send) -> String {
        let (console_tx, console_rx) = unbounded::<Arc<LogRecord>>();
        let (async_tx, _async_rx) = unbounded::<Arc<LogRecord>>();
        let metrics = Arc::new(Metrics::new());

        // Main thread subscriber
        let main_layer =
            LoggerSubscriber::new(console_tx.clone(), async_tx.clone(), metrics.clone())
                .with_filter(LevelFilter::DEBUG);
        let main_registry = tracing_subscriber::registry().with(main_layer);

        // Build a custom rayon thread pool that installs an inklog
        // LoggerSubscriber (sharing the same console channel) on each worker.
        let console_tx_for_handler = console_tx.clone();
        let async_tx_for_handler = async_tx.clone();
        let metrics_for_handler = metrics.clone();
        let pool = ThreadPoolBuilder::new()
            .start_handler(move |_idx| {
                let worker_layer = LoggerSubscriber::new(
                    console_tx_for_handler.clone(),
                    async_tx_for_handler.clone(),
                    metrics_for_handler.clone(),
                )
                .with_filter(LevelFilter::DEBUG);
                let worker_registry = tracing_subscriber::registry().with(worker_layer);
                let guard = tracing::subscriber::set_default(worker_registry);
                TRACING_GUARD.with(|g| *g.borrow_mut() = Some(guard));
            })
            .build()
            .expect("rayon thread pool");

        tracing::subscriber::with_default(main_registry, || {
            pool.install(f);
        });

        drain_to_string(&console_rx)
    }

    #[test]
    fn log_002_file_parsed_event_emitted_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![
            make_file(dir.path(), "a.rs", "fn foo() {}", Some(Language::Rust)),
            make_file(dir.path(), "b.rs", "fn bar() {}", Some(Language::Rust)),
        ];

        let captured = capture_tracing_debug(|| {
            let _ = parallel_parse(&files, "proj");
        });

        assert!(
            captured.contains("file_parsed"),
            "LOG-002: file_parsed event should be emitted, got: {captured:?}"
        );
        // Each successfully parsed file should emit an event.
        // Count occurrences of "file_parsed" in the captured output.
        let count = captured.matches("file_parsed").count();
        assert_eq!(
            count, 2,
            "LOG-002: one file_parsed event per parsed file, got {count}"
        );
        assert!(
            captured.contains("a.rs"),
            "file_parsed event should carry the file path"
        );
        assert!(
            captured.contains("nodes"),
            "file_parsed event should carry the nodes field"
        );
    }

    #[test]
    fn log_002_file_parsed_not_emitted_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        // A file that does not exist on disk triggers an Io error.
        let files = vec![FileInfo {
            path: dir.path().join("does_not_exist.rs"),
            relative_path: "does_not_exist.rs".to_string(),
            language: Some(Language::Rust),
            size: 0,
        }];

        let captured = capture_tracing_debug(|| {
            let _ = parallel_parse(&files, "proj");
        });

        assert!(
            !captured.contains("file_parsed"),
            "LOG-002: file_parsed should NOT be emitted on parse failure, got: {captured:?}"
        );
    }

    // -----------------------------------------------------------------------
    // parallel_parse_ram_first (H15/D9): LZ4-compressed in-memory parsing
    // -----------------------------------------------------------------------

    /// Helper: LZ4-compress a source string with the size prefix that
    /// `decompress_size_prepended` expects (4-byte LE original size + raw
    /// LZ4 block). `lz4_flex` 0.11 does not expose `compress_prepared`
    /// directly, so we assemble the frame manually.
    fn compress_source(source: &str) -> Vec<u8> {
        let compressed = lz4_flex::compress(source.as_bytes());
        let mut result = (source.len() as u32).to_le_bytes().to_vec();
        result.extend(compressed);
        result
    }

    #[test]
    fn ram_first_parses_from_compressed_buffer() {
        // Happy path: file is in the compressed map, decompress + parse.
        let dir = tempfile::tempdir().unwrap();
        let source = "fn foo() {}\nfn bar() {}\n";
        let file = make_file(dir.path(), "a.rs", source, Some(Language::Rust));

        let mut compressed = RamFirstSources::new();
        compressed.insert(file.path.clone(), compress_source(source));

        let result = parallel_parse_ram_first(&[file], &compressed, "proj");
        assert_eq!(result.files_parsed, 1, "got errors: {:?}", result.errors);
        assert_eq!(result.files_failed, 0);
        assert_eq!(result.results.len(), 1);
        let names: Vec<&str> = result.results[0]
            .nodes
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert!(names.contains(&"foo"), "should extract foo: {names:?}");
        assert!(names.contains(&"bar"), "should extract bar: {names:?}");
    }

    #[test]
    fn ram_first_falls_back_to_disk_when_not_in_map() {
        // File not in compressed map → fallback to disk read (still succeeds).
        let dir = tempfile::tempdir().unwrap();
        let file = make_file(dir.path(), "a.rs", "fn foo() {}", Some(Language::Rust));

        let compressed = RamFirstSources::new();
        let result = parallel_parse_ram_first(&[file], &compressed, "proj");
        assert_eq!(result.files_parsed, 1, "got errors: {:?}", result.errors);
        assert_eq!(result.files_failed, 0);
        assert_eq!(result.results.len(), 1);
    }

    #[test]
    fn ram_first_lz4_decompress_failure_is_collected() {
        // Corrupted LZ4 bytes → decompress fails → error collected, batch continues.
        let dir = tempfile::tempdir().unwrap();
        let file = make_file(dir.path(), "a.rs", "fn foo() {}", Some(Language::Rust));

        let mut compressed = RamFirstSources::new();
        compressed.insert(file.path.clone(), vec![0xFF; 8]);

        let result = parallel_parse_ram_first(&[file], &compressed, "proj");
        assert_eq!(result.files_parsed, 0);
        assert_eq!(result.files_failed, 1);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].0, "a.rs");
        assert!(
            result.errors[0].1.contains("LZ4 decompress failed"),
            "got: {}",
            result.errors[0].1
        );
    }

    #[test]
    fn ram_first_utf8_decode_failure_is_collected() {
        // Valid LZ4 but invalid UTF-8 → UTF-8 decode fails → error collected.
        let dir = tempfile::tempdir().unwrap();
        let file = make_file(dir.path(), "a.rs", "fn foo() {}", Some(Language::Rust));

        let invalid_utf8: Vec<u8> = vec![0xFF, 0xFE, 0xFD, 0x80];
        let mut compressed = RamFirstSources::new();
        let mut prepared = (invalid_utf8.len() as u32).to_le_bytes().to_vec();
        prepared.extend(lz4_flex::compress(&invalid_utf8));
        compressed.insert(file.path.clone(), prepared);

        let result = parallel_parse_ram_first(&[file], &compressed, "proj");
        assert_eq!(result.files_parsed, 0);
        assert_eq!(result.files_failed, 1);
        assert!(
            result.errors[0].1.contains("UTF-8 decode failed"),
            "got: {}",
            result.errors[0].1
        );
    }

    #[test]
    fn ram_first_unknown_language_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = make_file(dir.path(), "a.rs", "fn foo() {}", None);

        let compressed = RamFirstSources::new();
        let result = parallel_parse_ram_first(&[file], &compressed, "proj");
        assert_eq!(result.files_parsed, 0);
        assert_eq!(result.files_failed, 1);
        assert!(result.errors[0].1.contains("unknown language"));
    }

    #[test]
    fn ram_first_mixed_success_fallback_and_failure() {
        // One file in map (success), one not in map (fallback disk read),
        // one with corrupted LZ4 (failure). Verify all three paths in one batch.
        let dir = tempfile::tempdir().unwrap();
        let src_a = "fn a() {}";
        let file_a = make_file(dir.path(), "a.rs", src_a, Some(Language::Rust));
        let file_b = make_file(dir.path(), "b.rs", "fn b() {}", Some(Language::Rust));
        let file_c = make_file(dir.path(), "c.rs", "fn c() {}", Some(Language::Rust));

        let mut compressed = RamFirstSources::new();
        compressed.insert(file_a.path.clone(), compress_source(src_a));
        // file_b intentionally NOT in map → disk fallback
        compressed.insert(file_c.path.clone(), vec![0xFF; 4]); // corrupt LZ4

        let files = vec![file_a, file_b, file_c];
        let result = parallel_parse_ram_first(&files, &compressed, "proj");
        assert_eq!(
            result.files_parsed, 2,
            "a + b should parse; got: {:?}",
            result.errors
        );
        assert_eq!(result.files_failed, 1);
        assert_eq!(result.results.len(), 2);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].0, "c.rs");
    }

    #[test]
    fn ram_first_empty_file_list_returns_empty() {
        let compressed = RamFirstSources::new();
        let result = parallel_parse_ram_first(&[], &compressed, "proj");
        assert_eq!(result.files_parsed, 0);
        assert_eq!(result.files_failed, 0);
        assert!(result.results.is_empty());
        assert!(result.errors.is_empty());
    }

    // -----------------------------------------------------------------------
    // parse_single: IO error for missing file
    // -----------------------------------------------------------------------

    #[test]
    fn parse_single_io_error_for_missing_file() {
        let file = FileInfo {
            path: std::path::PathBuf::from("/nonexistent/path/missing.rs"),
            relative_path: "missing.rs".to_string(),
            language: Some(Language::Rust),
            size: 0,
        };
        let result = parse_single(&file, "proj");
        assert!(
            result.is_err(),
            "expected error for missing file, got: {result:?}"
        );
        match result.unwrap_err() {
            ParseError::Io { file_path, .. } => {
                assert!(
                    file_path.contains("missing.rs"),
                    "error should carry file path, got: {file_path}"
                );
            }
            other => panic!("expected ParseError::Io, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // LOG-002 (ram-first): file_parsed_ram_first event emission
    // -----------------------------------------------------------------------

    #[test]
    fn log_002_ram_first_emits_file_parsed_ram_first_event() {
        let dir = tempfile::tempdir().unwrap();
        let source = "fn foo() {}\nfn bar() {}\n";
        let file = make_file(dir.path(), "a.rs", source, Some(Language::Rust));

        let mut compressed = RamFirstSources::new();
        compressed.insert(file.path.clone(), compress_source(source));

        let captured = capture_tracing_debug(|| {
            let _ = parallel_parse_ram_first(&[file], &compressed, "proj");
        });

        assert!(
            captured.contains("file_parsed_ram_first"),
            "LOG-002: file_parsed_ram_first event should be emitted, got: {captured:?}"
        );
        assert!(
            captured.contains("a.rs"),
            "event should carry the file path, got: {captured:?}"
        );
        assert!(
            captured.contains("nodes"),
            "event should carry the nodes field, got: {captured:?}"
        );
    }

    #[test]
    fn log_002_ram_first_warns_on_disk_fallback() {
        // File NOT in compressed map → warn! about fallback should be captured.
        let dir = tempfile::tempdir().unwrap();
        let file = make_file(dir.path(), "a.rs", "fn foo() {}", Some(Language::Rust));

        let compressed = RamFirstSources::new();
        let captured = capture_tracing_debug(|| {
            let _ = parallel_parse_ram_first(&[file], &compressed, "proj");
        });

        assert!(
            captured.contains("RAM-first"),
            "warn event should mention RAM-first, got: {captured:?}"
        );
        assert!(
            captured.contains("falling back to disk read"),
            "warn event should mention disk fallback, got: {captured:?}"
        );
        assert!(
            captured.contains("a.rs"),
            "warn event should carry the file path, got: {captured:?}"
        );
    }

    #[test]
    fn log_002_ram_first_no_event_on_decompress_failure() {
        // Corrupted LZ4 → decompress fails → file_parsed_ram_first should NOT
        // be emitted (failure path, not success path).
        let dir = tempfile::tempdir().unwrap();
        let file = make_file(dir.path(), "a.rs", "fn foo() {}", Some(Language::Rust));

        let mut compressed = RamFirstSources::new();
        compressed.insert(file.path.clone(), vec![0xFF; 8]);

        let captured = capture_tracing_debug(|| {
            let _ = parallel_parse_ram_first(&[file], &compressed, "proj");
        });

        assert!(
            !captured.contains("file_parsed_ram_first"),
            "file_parsed_ram_first should NOT be emitted on decompress failure, got: {captured:?}"
        );
    }

    // -----------------------------------------------------------------------
    // parallel_parse_ram_first: multiple files in compressed map
    // -----------------------------------------------------------------------

    #[test]
    fn ram_first_multiple_compressed_files_all_parsed() {
        let dir = tempfile::tempdir().unwrap();
        let src_a = "fn a() {}";
        let src_b = "fn b() {}\nfn c() {}\n";
        let file_a = make_file(dir.path(), "a.rs", src_a, Some(Language::Rust));
        let file_b = make_file(dir.path(), "b.rs", src_b, Some(Language::Rust));

        let mut compressed = RamFirstSources::new();
        compressed.insert(file_a.path.clone(), compress_source(src_a));
        compressed.insert(file_b.path.clone(), compress_source(src_b));

        let result = parallel_parse_ram_first(&[file_a, file_b], &compressed, "proj");
        assert_eq!(result.files_parsed, 2, "got errors: {:?}", result.errors);
        assert_eq!(result.files_failed, 0);
        assert_eq!(result.results.len(), 2);
        // Verify all function names are extracted.
        let all_names: Vec<&str> = result
            .results
            .iter()
            .flat_map(|r| r.nodes.iter().map(|n| n.name.as_str()))
            .collect();
        assert!(all_names.contains(&"a"), "should extract a: {all_names:?}");
        assert!(all_names.contains(&"b"), "should extract b: {all_names:?}");
        assert!(all_names.contains(&"c"), "should extract c: {all_names:?}");
    }

    #[test]
    fn ram_first_all_files_failed_returns_empty_results() {
        // All files have corrupted LZ4 → all fail → empty results.
        let dir = tempfile::tempdir().unwrap();
        let file_a = make_file(dir.path(), "a.rs", "fn foo() {}", Some(Language::Rust));
        let file_b = make_file(dir.path(), "b.rs", "fn bar() {}", Some(Language::Rust));

        let mut compressed = RamFirstSources::new();
        compressed.insert(file_a.path.clone(), vec![0xFF; 4]);
        compressed.insert(file_b.path.clone(), vec![0xFF; 4]);

        let result = parallel_parse_ram_first(&[file_a, file_b], &compressed, "proj");
        assert_eq!(result.files_parsed, 0);
        assert_eq!(result.files_failed, 2);
        assert!(result.results.is_empty());
        assert_eq!(result.errors.len(), 2);
        // Every error should mention LZ4 decompress failure.
        for (_, msg) in &result.errors {
            assert!(
                msg.contains("LZ4 decompress failed"),
                "expected LZ4 failure, got: {msg}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // L2: mpsc channel + par_chunks streaming — large-batch coverage
    //
    // These tests exercise the streaming/backpressure path that replaced the
    // pre-L2 `par_iter().collect()` shape. They use batches larger than the
    // channel bound and chunk size to ensure results are not lost when the
    // main thread drains the channel slowly.
    // -----------------------------------------------------------------------

    /// Builds `count` Rust files with unique function names `f0`, `f1`, ...
    /// and inserts each into `compressed` with LZ4-prepared bytes.
    fn make_compressed_batch(
        dir: &std::path::Path,
        count: usize,
    ) -> (Vec<FileInfo>, RamFirstSources) {
        let mut files = Vec::with_capacity(count);
        let mut compressed = RamFirstSources::new();
        for i in 0..count {
            let name = format!("f{i}.rs");
            let source = format!("fn f{i}() {{}}");
            let file = make_file(dir, &name, &source, Some(Language::Rust));
            compressed.insert(file.path.clone(), compress_source(&source));
            files.push(file);
        }
        (files, compressed)
    }

    #[test]
    fn ram_first_large_batch_all_parsed_under_backpressure() {
        // 64 files > CHANNEL_BOUND (4) × typical chunk size, exercising the
        // backpressure path: workers must block on send when the channel is
        // full and the main thread is still draining.
        let dir = tempfile::tempdir().unwrap();
        let (files, compressed) = make_compressed_batch(dir.path(), 64);

        let result = parallel_parse_ram_first(&files, &compressed, "proj");
        assert_eq!(result.files_failed, 0, "got errors: {:?}", result.errors);
        assert_eq!(result.files_parsed, 64);
        assert_eq!(result.results.len(), 64);
        assert!(result.errors.is_empty());

        // Every f0..f63 must appear exactly once across all results.
        // Use a HashSet to validate uniqueness + completeness without
        // depending on lexicographic vs numeric ordering.
        let names: std::collections::HashSet<String> = result
            .results
            .iter()
            .flat_map(|r| r.nodes.iter().map(|n| n.name.clone()))
            .collect();
        assert_eq!(
            names.len(),
            64,
            "no duplicate names expected, got: {names:?}"
        );
        for i in 0..64 {
            assert!(
                names.contains(&format!("f{i}")),
                "missing f{i} in extracted names: {names:?}"
            );
        }
    }

    #[test]
    fn ram_first_large_batch_mixed_success_failure_keeps_exact_counts() {
        // Interleave success (in compressed map) with failure (corrupt LZ4)
        // across a 48-file batch. The streaming path must not lose or
        // duplicate any result regardless of which worker handled it.
        let dir = tempfile::tempdir().unwrap();
        let mut files = Vec::with_capacity(48);
        let mut compressed = RamFirstSources::new();
        for i in 0..48 {
            let name = format!("m{i}.rs");
            let source = format!("fn m{i}() {{}}");
            let file = make_file(dir.path(), &name, &source, Some(Language::Rust));
            if i % 2 == 0 {
                compressed.insert(file.path.clone(), compress_source(&source));
            } else {
                // Odd index → corrupt LZ4 → guaranteed decompress failure.
                compressed.insert(file.path.clone(), vec![0xFF; 8]);
            }
            files.push(file);
        }

        let result = parallel_parse_ram_first(&files, &compressed, "proj");
        assert_eq!(result.files_parsed, 24, "got errors: {:?}", result.errors);
        assert_eq!(result.files_failed, 24);
        assert_eq!(result.results.len(), 24);
        assert_eq!(result.errors.len(), 24);
        // All failures must mention LZ4 decompress failure.
        for (_, msg) in &result.errors {
            assert!(
                msg.contains("LZ4 decompress failed"),
                "expected LZ4 failure, got: {msg}"
            );
        }
        // parsed + failed must equal input length (no result lost).
        assert_eq!(
            result.files_parsed + result.files_failed,
            files.len(),
            "streaming must not drop results"
        );
    }

    #[test]
    fn ram_first_large_batch_with_disk_fallback_completes() {
        // 40 files: half in compressed map (RAM-first), half missing from
        // the map (disk fallback). Verifies the streaming path correctly
        // handles the fallback branch under backpressure.
        let dir = tempfile::tempdir().unwrap();
        let mut files = Vec::with_capacity(40);
        let mut compressed = RamFirstSources::new();
        for i in 0..40 {
            let name = format!("d{i}.rs");
            let source = format!("fn d{i}() {{}}");
            let file = make_file(dir.path(), &name, &source, Some(Language::Rust));
            if i % 2 == 0 {
                compressed.insert(file.path.clone(), compress_source(&source));
            }
            // Odd index intentionally missing from `compressed` → disk read.
            files.push(file);
        }

        let result = parallel_parse_ram_first(&files, &compressed, "proj");
        assert_eq!(result.files_failed, 0, "got errors: {:?}", result.errors);
        assert_eq!(result.files_parsed, 40);
        assert_eq!(result.results.len(), 40);
    }

    #[test]
    fn ram_first_single_file_uses_streaming_path_correctly() {
        // Boundary: batch size = 1 must still work after the streaming
        // refactor (no off-by-one in chunking or channel drain).
        let dir = tempfile::tempdir().unwrap();
        let source = "fn lone() {}";
        let file = make_file(dir.path(), "lone.rs", source, Some(Language::Rust));

        let mut compressed = RamFirstSources::new();
        compressed.insert(file.path.clone(), compress_source(source));

        let result = parallel_parse_ram_first(&[file], &compressed, "proj");
        assert_eq!(result.files_parsed, 1);
        assert_eq!(result.files_failed, 0);
        assert_eq!(result.results.len(), 1);
        let names: Vec<&str> = result.results[0]
            .nodes
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert!(names.contains(&"lone"), "should extract lone: {names:?}");
    }
}
