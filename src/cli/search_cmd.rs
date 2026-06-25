//! `search` subcommand handler (PRD §4.4).
//!
//! Calls [`QueryFacade::search`] (or [`QueryFacade::fulltext_search`] when
//! `--semantic` is set) and prints the results as a JSON array.
//!
//! When the `embed` feature is enabled and `--semantic` is set, the command
//! uses [`HybridStrategy`] (BM25 + vector RRF fusion, AC-SEARCH-002) if an
//! embedding API key is configured; otherwise it falls back to BM25 full-text
//! search.

use serde::Serialize;

use super::args::SearchArgs;
use super::error::Result;
use crate::query::{QueryFacade, SearchResult};

/// Runs the `search` subcommand.
///
/// Opens the database at `args.db`, runs the search, and prints the results
/// as a JSON array of [`SearchResultOutput`] objects.
///
/// # Errors
///
/// Returns [`crate::cli::error::CliError::Query`] for search failures, or
/// [`crate::cli::error::CliError::Storage`] if the database cannot be opened.
pub fn run(args: &SearchArgs) -> Result<()> {
    let db_path = std::path::Path::new(&args.db);
    let facade = QueryFacade::new(db_path)?;
    let results = if args.semantic {
        semantic_search(&facade, &args.text, args.limit)?
    } else {
        facade.search(&args.text, None, args.limit)?
    };
    let output: Vec<SearchResultOutput> =
        results.into_iter().map(SearchResultOutput::from).collect();
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// Executes a semantic search, using the embed subsystem when available.
///
/// When the `embed` feature is enabled and an API key is configured, this uses
/// [`HybridStrategy`] (BM25 + vector RRF fusion). Otherwise it falls back to
/// BM25 full-text search via [`QueryFacade::fulltext_search`].
fn semantic_search(facade: &QueryFacade, text: &str, limit: usize) -> Result<Vec<SearchResult>> {
    #[cfg(feature = "embed")]
    {
        use crate::embed::{EmbeddingConfig, HybridStrategy, OpenAIEmbedClient, SearchStrategy};

        let config = EmbeddingConfig::from_env();
        if config.has_api_key() {
            if let Ok(client) = OpenAIEmbedClient::new(config) {
                let strategy = HybridStrategy::new(facade.connection(), &client);
                if let Ok(results) = strategy.search(text, None, limit) {
                    return Ok(results);
                }
            }
        }
    }
    // Fallback: BM25 full-text search (always available).
    Ok(facade.fulltext_search(text, None, limit)?)
}

/// JSON-serializable view of a single search result.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SearchResultOutput {
    /// Short display name of the matched symbol.
    pub name: String,
    /// Node label (e.g. `"Function"`).
    pub label: String,
    /// Source file path, when available.
    pub file_path: Option<String>,
    /// 1-based start line, when available.
    pub start_line: Option<u32>,
    /// Fully qualified name, when available.
    pub qualified_name: Option<String>,
    /// Relevance score in `[0.0, 1.0]`.
    pub score: f32,
}

impl From<SearchResult> for SearchResultOutput {
    fn from(r: SearchResult) -> Self {
        Self {
            name: r.name,
            label: r.label,
            file_path: r.file_path,
            start_line: r.start_line,
            qualified_name: r.qualified_name,
            score: r.score,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::SearchArgs;
    use crate::storage::StorageConnection;
    use std::path::Path;
    use tempfile::TempDir;

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cli_search_testdb");
        std::mem::forget(dir);
        path
    }

    /// Seeds the database with functions whose names contain "parse".
    fn seed_search_fixture(db: &Path) {
        let conn = StorageConnection::open(db).expect("open");
        conn.init_schema().expect("init_schema");
        conn.execute("CREATE (:Project {id: 'demo', name: 'demo', rootPath: '/', language: 'rust', fileCount: 2, indexedAt: 0});").expect("create project");
        conn.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'parse_file', qualifiedName: 'demo.parse_file', filePath: '/src/main.rs', startLine: 1, endLine: 10, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create f1");
        conn.execute("CREATE (:Function {id: 'f2', project: 'demo', name: 'parse_line', qualifiedName: 'demo.parse_line', filePath: '/src/main.rs', startLine: 11, endLine: 20, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create f2");
        conn.execute("CREATE (:Function {id: 'f3', project: 'demo', name: 'read_input', qualifiedName: 'demo.read_input', filePath: '/src/lib.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create f3");
    }

    fn make_args(text: &str, semantic: bool, limit: usize, db: &str) -> SearchArgs {
        SearchArgs {
            text: text.to_string(),
            semantic,
            limit,
            db: db.to_string(),
        }
    }

    // --- SearchResultOutput ---

    #[test]
    fn search_result_output_from_search_result() {
        let r = SearchResult {
            name: "parse".into(),
            label: "Function".into(),
            file_path: Some("/x.rs".into()),
            start_line: Some(1),
            qualified_name: Some("demo.parse".into()),
            score: 0.9,
        };
        let out = SearchResultOutput::from(r);
        assert_eq!(out.name, "parse");
        assert_eq!(out.label, "Function");
        assert_eq!(out.file_path.as_deref(), Some("/x.rs"));
        assert_eq!(out.start_line, Some(1));
        assert_eq!(out.qualified_name.as_deref(), Some("demo.parse"));
        assert!((out.score - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn search_result_output_serializes_to_json() {
        let out = SearchResultOutput {
            name: "x".into(),
            label: "Function".into(),
            file_path: None,
            start_line: None,
            qualified_name: None,
            score: 0.5,
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"name\":\"x\""));
        assert!(json.contains("\"label\":\"Function\""));
    }

    // --- run() success ---

    #[test]
    fn run_search_returns_results() {
        let db = fresh_db_path();
        seed_search_fixture(&db);
        let args = make_args("parse", false, 10, db.to_str().unwrap());
        let result = run(&args);
        assert!(result.is_ok(), "search should succeed: {:?}", result.err());
    }

    #[test]
    fn run_search_semantic_uses_fulltext() {
        let db = fresh_db_path();
        seed_search_fixture(&db);
        let args = make_args("parse", true, 10, db.to_str().unwrap());
        let result = run(&args);
        assert!(
            result.is_ok(),
            "semantic search should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn run_search_no_matches_succeeds() {
        let db = fresh_db_path();
        seed_search_fixture(&db);
        let args = make_args("zzz_nonexistent", false, 10, db.to_str().unwrap());
        let result = run(&args);
        assert!(
            result.is_ok(),
            "no-match search should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn run_search_limit_one_succeeds() {
        let db = fresh_db_path();
        seed_search_fixture(&db);
        let args = make_args("parse", false, 1, db.to_str().unwrap());
        let result = run(&args);
        assert!(
            result.is_ok(),
            "limit-1 search should succeed: {:?}",
            result.err()
        );
    }

    // --- run() error cases ---

    #[test]
    fn run_search_missing_db_returns_error() {
        let args = make_args("parse", false, 10, "/nonexistent/db.lbug");
        let result = run(&args);
        assert!(result.is_err(), "missing db should error");
    }

    #[test]
    fn run_search_empty_db_returns_empty_array() {
        let db = fresh_db_path();
        // Initialize schema but seed nothing.
        let conn = StorageConnection::open(&db).expect("open");
        conn.init_schema().expect("init_schema");
        drop(conn);
        let args = make_args("parse", false, 10, db.to_str().unwrap());
        let result = run(&args);
        assert!(
            result.is_ok(),
            "empty-db search should succeed: {:?}",
            result.err()
        );
    }
}
