// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! BM25F field-weighted scoring (C4 upgrade from BM25).
//!
//! Provides [`FieldWeights`] and [`bm25f_score`] for multi-field weighted
//! relevance scoring. Each field (`symbol_name`, `qualified_name`, `docstring`,
//! `content`) is scored independently using the same relevance rules as the
//! single-field BM25 fallback, then combined via a weighted sum.
//!
//! # Default weights
//!
//! `symbol_name` (3.5) > `function_name` (2.0) > `comment` (0.5) > `string_literal` (0.3)
//!
//! This ensures a name match on `parse` ranks higher than a docstring match on
//! `parse`, fixing the BM25 single-field limitation where `name="parse"` and
//! `docstring="parse me"` would receive identical scores.
//!
//! # Scope
//!
//! Only the [`super::fulltext`] fallback (CONTAINS scan) path uses BM25F. The
//! LadybugDB FTS extension path remains single-field BM25 because the FTS
//! extension does not expose per-field weights.

use super::tokenizer::codenexus_tokenize;

/// Per-field weights for BM25F multi-field scoring.
///
/// Weights are multipliers applied to each field's normalised relevance score
/// (in `[0.0, 1.0]`). The final BM25F score is the weighted sum:
///
/// ```text
/// score = weights.symbol_name     * relevance(name)
///       + weights.function_name   * relevance(qualified_name)
///       + weights.comment         * relevance(docstring)
///       + weights.string_literal  * relevance(content)
/// ```
///
/// Fields with no match contribute `0.0` (the `0.3` "no match" floor from
/// [`relevance_score_with_reason`] is treated as zero contribution).
///
/// # Defaults
///
/// - `symbol_name: 3.5` — highest weight, primary user-facing identifier
/// - `function_name: 2.0` — qualified name (e.g. `demo.parse_file`)
/// - `comment: 0.5` — docstring/comment text
/// - `string_literal: 0.3` — content/code body (lowest signal-to-noise)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FieldWeights {
    /// Weight for the `name` field (short display name).
    pub symbol_name: f64,
    /// Weight for the `qualifiedName` field (fully qualified name).
    pub function_name: f64,
    /// Weight for the `docstring` field (docstring/comment).
    pub comment: f64,
    /// Weight for the `content` field (source body / string literals).
    pub string_literal: f64,
}

impl Default for FieldWeights {
    fn default() -> Self {
        Self {
            symbol_name: 3.5,
            function_name: 2.0,
            comment: 0.5,
            string_literal: 0.3,
        }
    }
}

/// Computes a relevance score in `[0.0, 1.0]` for `name` against `query`.
///
/// Scoring tiers (H11 token-aware):
/// - `1.0` — exact match (case-insensitive)
/// - `0.8` — name starts with query (prefix)
/// - `0.7` — every query token appears as a substring of some name token
///   (e.g. `my_parse_helper` vs `parse` → `["my","parse","helper"]` contains
///   `parse`)
/// - `0.5` — name contains query as a plain substring
/// - `0.3` — no match (defensive; callers pre-filter via `CONTAINS`)
///
/// Tokenization must receive the **original-case** inputs, not pre-lowercased
/// ones: [`codenexus_tokenize`] uses uppercase letters as camelCase boundary
/// markers (e.g. `parseFile` → `["parse", "file"]`) and case-folds the output
/// itself via `fold_case`. Pre-lowering collapses `parseFile` to `parsefile`,
/// destroying the boundary and yielding a single token, which made
/// `parse_file` (snake_case) unmatchable against the camelCase query
/// `parseFile` (C4 regression).
pub(crate) fn relevance_score_with_reason(name: &str, query: &str) -> (f64, &'static str) {
    let name_lower = name.to_ascii_lowercase();
    let query_lower = query.to_ascii_lowercase();
    if name_lower == query_lower {
        return (1.0, "exact name match");
    }
    if name_lower.starts_with(&query_lower) {
        return (0.8, "prefix match");
    }
    // Tokenize the ORIGINAL (case-preserving) inputs: the tokenizer needs
    // uppercase letters to detect camelCase/PascalCase boundaries and
    // performs its own case folding on the output tokens.
    let query_tokens = codenexus_tokenize(query);
    let name_tokens = codenexus_tokenize(name);
    if !query_tokens.is_empty() && !name_tokens.is_empty() {
        let all_match = query_tokens
            .iter()
            .all(|qt| name_tokens.iter().any(|nt| nt == qt));
        if all_match {
            return (0.7, "token-aligned match");
        }
    }
    if name_lower.contains(&query_lower) {
        return (0.5, "substring match");
    }
    (0.3, "no match")
}

/// BM25F field-level score: same tiers as [`relevance_score_with_reason`] but
/// returns `0.0` for no match (instead of the `0.3` defensive baseline).
///
/// BM25F sums weighted field scores; a no-match field must contribute `0.0`
/// otherwise non-matching symbols would receive a non-zero total.
fn bm25f_field_score(field: &str, query: &str) -> f64 {
    let (score, _) = relevance_score_with_reason(field, query);
    // `0.3` is the "no match" floor from `relevance_score_with_reason`.
    // BM25F treats no-match fields as zero contribution.
    if score <= 0.3 {
        0.0
    } else {
        score
    }
}

/// Computes a BM25F multi-field weighted score for `query` against a symbol.
///
/// Each field is scored independently via [`bm25f_field_score`] (in `[0.0, 1.0]`,
/// with `0.0` for no match), then multiplied by its weight from `weights` and
/// summed.
///
/// Returns `0.0` when `query` is empty or whitespace-only (defensive; the
/// search API rejects empty queries, but `bm25f_score` is `pub` and may be
/// called directly).
///
/// # Arguments
///
/// * `query` — search query (e.g. `"parse"`)
/// * `symbol_name` — short display name (e.g. `"parse_file"`)
/// * `qualified_name` — fully qualified name (e.g. `"demo.parse_file"`)
/// * `docstring` — docstring/comment text (may be empty)
/// * `content` — content/code body (may be empty)
/// * `weights` — per-field weights
///
/// # Examples
///
/// ```
/// use codenexus::query::bm25f::{bm25f_score, FieldWeights};
///
/// let w = FieldWeights::default();
/// // Exact name match dominates comment substring match.
/// let name_exact = bm25f_score("parse", "parse", "demo.parse", "", "", &w);
/// let comment_sub = bm25f_score("parse", "read", "demo.read", "parse a file", "", &w);
/// assert!(name_exact > comment_sub);
/// ```
#[must_use]
pub fn bm25f_score(
    query: &str,
    symbol_name: &str,
    qualified_name: &str,
    docstring: &str,
    content: &str,
    weights: &FieldWeights,
) -> f64 {
    if query.trim().is_empty() {
        return 0.0;
    }
    // Each field is scored independently; no-match fields contribute 0.
    weights.symbol_name * bm25f_field_score(symbol_name, query)
        + weights.function_name * bm25f_field_score(qualified_name, query)
        + weights.comment * bm25f_field_score(docstring, query)
        + weights.string_literal * bm25f_field_score(content, query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Language, Node, NodeLabel};
    use crate::query::FullTextSearcher;
    use crate::storage::Repository;

    // === Unit tests for FieldWeights ===

    #[test]
    fn test_bm25f_field_weights_default_values() {
        let w = FieldWeights::default();
        assert_eq!(w.symbol_name, 3.5);
        assert_eq!(w.function_name, 2.0);
        assert_eq!(w.comment, 0.5);
        assert_eq!(w.string_literal, 0.3);
    }

    #[test]
    fn test_bm25f_field_weights_is_copy_and_eq() {
        // FieldWeights derives Copy + PartialEq — passing by value should not
        // move the original, and equal weights should compare equal.
        let w1 = FieldWeights::default();
        let w2 = w1; // Copy
        assert_eq!(w1, w2);
        let w3 = FieldWeights {
            symbol_name: 1.0,
            ..FieldWeights::default()
        };
        assert_ne!(w1, w3);
    }

    // === Unit tests for bm25f_score (pure function) ===

    #[test]
    fn test_bm25f_empty_query_returns_zero() {
        let w = FieldWeights::default();
        let score = bm25f_score("", "parse", "demo.parse", "parse a file", "content", &w);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_bm25f_whitespace_only_query_returns_zero() {
        let w = FieldWeights::default();
        let score = bm25f_score("   \t\n", "parse", "demo.parse", "doc", "content", &w);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_bm25f_no_match_returns_zero() {
        // No field contains "parse" → all fields score 0.3 (no match) →
        // bm25f_field_score returns 0.0 for each → total 0.0.
        let w = FieldWeights::default();
        let score = bm25f_score(
            "parse",
            "read_input",
            "demo.read_input",
            "read user input",
            "stdin content",
            &w,
        );
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_bm25f_exact_match_in_symbol_name_beats_substring_in_comment() {
        let w = FieldWeights::default();
        // Symbol with exact name match, no other field matches.
        let score_name = bm25f_score("parse", "parse", "demo.parse", "", "", &w);
        // Symbol with no name match but comment substring match.
        let score_comment = bm25f_score(
            "parse",
            "read_input",
            "demo.read_input",
            "parse a file",
            "",
            &w,
        );
        assert!(
            score_name > score_comment,
            "exact name match ({score_name}) should beat comment substring ({score_comment})"
        );
        // Sanity: exact name match should be non-zero.
        assert!(score_name > 0.0);
        // Sanity: comment-only match should be non-zero (comment weight * 0.5).
        assert!(score_comment > 0.0);
    }

    #[test]
    fn test_bm25f_multiple_fields_same_symbol_sums_weights() {
        let w = FieldWeights::default();
        // Symbol where both name and qualifiedName match exactly.
        let score_both = bm25f_score("parse", "parse", "parse", "", "", &w);
        // Symbol where only name matches.
        let score_name_only = bm25f_score("parse", "parse", "demo.other", "", "", &w);
        assert!(
            score_both > score_name_only,
            "multi-field match ({score_both}) should sum and exceed single-field ({score_name_only})"
        );
    }

    #[test]
    fn test_bm25f_custom_weights_change_ranking() {
        // With comment weight >> symbol_name weight, a comment match should
        // outscore a name match.
        let w = FieldWeights {
            symbol_name: 0.1,
            function_name: 0.1,
            comment: 10.0,
            string_literal: 0.1,
        };
        let score_name = bm25f_score("parse", "parse", "demo.parse", "", "", &w);
        let score_comment = bm25f_score("parse", "read", "demo.read", "parse a file", "", &w);
        assert!(
            score_comment > score_name,
            "with comment weight 10.0, comment match ({score_comment}) should beat name match ({score_name})"
        );
    }

    #[test]
    fn test_bm25f_zero_weights_returns_zero() {
        // All-zero weights → score is always 0 regardless of matches.
        let w = FieldWeights {
            symbol_name: 0.0,
            function_name: 0.0,
            comment: 0.0,
            string_literal: 0.0,
        };
        let score = bm25f_score("parse", "parse", "demo.parse", "parse", "parse", &w);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_bm25f_content_field_contributes_via_string_literal_weight() {
        // content (string_literal) field match should contribute its weight.
        let w = FieldWeights::default();
        let score_no_content = bm25f_score("parse", "parse", "demo.parse", "", "", &w);
        let score_with_content = bm25f_score("parse", "parse", "demo.parse", "", "parse code", &w);
        assert!(
            score_with_content > score_no_content,
            "content match should add to score: {score_with_content} vs {score_no_content}"
        );
    }

    // === Unit tests for relevance_score_with_reason ===

    #[test]
    fn relevance_score_with_reason_exact_match_returns_one() {
        let (score, reason) = relevance_score_with_reason("parse", "parse");
        assert_eq!(score, 1.0);
        assert_eq!(reason, "exact name match");
        // Case-insensitive: uppercase name matches lowercase query.
        let (score, _) = relevance_score_with_reason("PARSE", "parse");
        assert_eq!(score, 1.0);
    }

    #[test]
    fn relevance_score_with_reason_prefix_match_returns_zero_eight() {
        let (score, reason) = relevance_score_with_reason("parse_file", "parse");
        assert_eq!(score, 0.8);
        assert_eq!(reason, "prefix match");
    }

    #[test]
    fn relevance_score_with_reason_token_aligned_match_returns_zero_seven() {
        let (score, reason) = relevance_score_with_reason("my_parse_helper", "parse");
        assert_eq!(score, 0.7);
        assert_eq!(reason, "token-aligned match");
    }

    #[test]
    fn relevance_score_with_reason_substring_match_returns_zero_five() {
        let (score, reason) = relevance_score_with_reason("myparsehelper", "parse");
        assert_eq!(score, 0.5);
        assert_eq!(reason, "substring match");
    }

    #[test]
    fn relevance_score_with_reason_no_match_returns_zero_three() {
        let (score, reason) = relevance_score_with_reason("read_input", "parse");
        assert_eq!(score, 0.3);
        assert_eq!(reason, "no match");
    }

    #[test]
    fn relevance_score_with_reason_camel_case_token_alignment() {
        // camelCase "parseFile" tokenized as ["parse","file"],
        // query "fileparse" tokenized as ["fileparse"] → not all tokens
        // match → falls through to substring → 0.3
        let (score, _) = relevance_score_with_reason("parseFile", "fileparse");
        assert_eq!(score, 0.3);
    }

    #[test]
    fn relevance_score_with_reason_camel_case_query_matches_snake_case_name() {
        // Regression (C4): camelCase query "parseFile" must match snake_case
        // name "parse_file" via token alignment. Pre-lowering the query
        // destroyed camelCase boundaries and produced a single "parsefile"
        // token, missing "parse_file" entirely.
        let (score, reason) = relevance_score_with_reason("parse_file", "parseFile");
        assert_eq!(
            score, 0.7,
            "camelCase query must match snake_case name via token alignment"
        );
        assert_eq!(reason, "token-aligned match");
    }

    #[test]
    fn relevance_score_with_reason_pascal_case_query_matches_snake_case_name() {
        // PascalCase query "ParseFile" also tokenizes to ["parse","file"]
        // and must match "parse_file".
        let (score, _) = relevance_score_with_reason("parse_file", "ParseFile");
        assert_eq!(score, 0.7);
    }

    #[test]
    fn relevance_score_with_reason_snake_case_query_matches_camel_case_name() {
        // Symmetric case: snake_case query "parse_file" matches camelCase
        // name "parseFile" via token alignment.
        let (score, _) = relevance_score_with_reason("parseFile", "parse_file");
        assert_eq!(score, 0.7);
    }

    #[test]
    fn relevance_score_with_reason_mixed_case_query_partial_token_match() {
        // camelCase query "parseFileBig" tokenizes to ["parse","file","big"].
        // Name "parse_file" tokenizes to ["parse","file"]. Not all query
        // tokens match (missing "big") → not token-aligned → no substring
        // either → 0.3.
        let (score, _) = relevance_score_with_reason("parse_file", "parseFileBig");
        assert_eq!(score, 0.3);
    }

    #[test]
    fn relevance_score_with_reason_name_with_empty_tokens() {
        // When the name is all digits, codenexus_tokenize returns empty →
        // the token-alignment check is skipped → falls through to substring
        // → 0.3 (since "12345" doesn't contain "parse").
        let (score, reason) = relevance_score_with_reason("12345", "parse");
        assert_eq!(score, 0.3);
        assert_eq!(reason, "no match");
    }

    #[test]
    fn relevance_score_with_reason_query_with_empty_tokens() {
        // When query_tokens is empty (query is all digits), token-alignment
        // is skipped → substring fails → 0.3.
        let (score, reason) = relevance_score_with_reason("parse", "12345");
        assert_eq!(score, 0.3);
        assert_eq!(reason, "no match");
    }

    // === Integration tests through FullTextSearcher ===

    fn fresh_repo() -> Repository {
        Repository::in_memory().expect("in_memory repository")
    }

    /// Builds a Function node with explicit docstring. `content` is set via
    /// the `properties` JSON bag (the schema stores `content` there).
    fn sample_function_with_docstring(
        id: &str,
        project: &str,
        name: &str,
        qn: &str,
        file: &str,
        line: u32,
        docstring: &str,
        content: &str,
    ) -> Node {
        let mut builder = Node::builder(NodeLabel::Function, name, qn)
            .id(id)
            .project(project)
            .file_path(file)
            .start_line(line)
            .end_line(line + 10)
            .language(Language::Rust)
            .signature("fn x()");
        if !docstring.is_empty() {
            builder = builder.docstring(docstring);
        }
        if !content.is_empty() {
            builder = builder.properties(serde_json::json!({ "content": content }));
        }
        builder.build()
    }

    #[test]
    fn test_bm25f_field_weights_boost_symbol_name_matches() {
        // Construct 10 Function symbols:
        //   - 5 with "parse" in symbol_name (no docstring, no content)
        //   - 5 with "parse" only in docstring (name sorts BEFORE "parse_*"
        //     alphabetically, so without BM25F weighting the comment-only
        //     matches would rank in TOP-5 by alphabetical tiebreak)
        // With BM25F + default weights, symbol_name matches (weight 3.5)
        // must outscore comment-only matches (weight 0.5).
        let repo = fresh_repo();
        let mut nodes = Vec::new();
        // 5 symbols with name match (sort AFTER "aaa_*" alphabetically)
        for i in 0..5 {
            nodes.push(sample_function_with_docstring(
                &format!("name_{i}"),
                "demo",
                &format!("parse_{i}"),
                &format!("demo.parse_{i}"),
                "/a.rs",
                i as u32 + 1,
                "",
                "",
            ));
        }
        // 5 symbols with comment-only match (sort BEFORE "parse_*" alphabetically)
        for i in 0..5 {
            nodes.push(sample_function_with_docstring(
                &format!("doc_{i}"),
                "demo",
                &format!("aaa_{i}"),
                &format!("demo.aaa_{i}"),
                "/b.rs",
                i as u32 + 100,
                &format!("parse something {i}"),
                "",
            ));
        }
        repo.save_nodes(&nodes, NodeLabel::Function)
            .expect("save_nodes");

        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher.search("parse", None, 100).expect("search");
        // Top-5 must all be symbol_name matches (parse_0..parse_4).
        let top5: Vec<&str> = results.iter().take(5).map(|r| r.name.as_str()).collect();
        assert!(
            !top5.is_empty(),
            "expected at least one result, got empty list"
        );
        for name in &top5 {
            assert!(
                name.starts_with("parse_"),
                "expected symbol_name match in TOP-5, got {name:?}; full top-5: {top5:?}"
            );
        }
    }

    #[test]
    fn test_bm25f_with_weights_builder_changes_ranking() {
        // Verify that FieldWeights can be overridden via the `with_weights`
        // builder. With comment weight >> symbol_name weight, comment-only
        // matches should outscore name matches.
        let repo = fresh_repo();
        let mut nodes = Vec::new();
        // name match, no docstring
        nodes.push(sample_function_with_docstring(
            "n1",
            "demo",
            "parse",
            "demo.parse",
            "/a.rs",
            1,
            "",
            "",
        ));
        // comment-only match
        nodes.push(sample_function_with_docstring(
            "n2",
            "demo",
            "other",
            "demo.other",
            "/b.rs",
            2,
            "parse a file",
            "",
        ));
        repo.save_nodes(&nodes, NodeLabel::Function)
            .expect("save_nodes");

        let weights = FieldWeights {
            symbol_name: 0.1,
            function_name: 0.1,
            comment: 10.0,
            string_literal: 0.1,
        };
        let searcher = FullTextSearcher::new(repo.connection()).with_weights(weights);
        let results = searcher.search("parse", None, 100).expect("search");
        assert_eq!(
            results.len(),
            2,
            "expected 2 results, got {}",
            results.len()
        );
        // With comment weight 10.0 >> symbol_name 0.1, comment-only match
        // should rank first.
        assert_eq!(
            results[0].name, "other",
            "expected comment-only match to rank first with weights {:?}, got top: {:?}",
            weights, results[0]
        );
    }

    #[test]
    fn test_bm25f_search_returns_zero_score_for_no_field_match() {
        // When a row is returned by CONTAINS but no field actually matches
        // the query token (defensive — shouldn't normally happen), the score
        // should still be 0.0 (not a phantom non-zero from the 0.3 floor).
        // This is implicitly covered by `test_bm25f_no_match_returns_zero`
        // at the unit level; here we verify the integration behaviour.
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function_with_docstring(
                "f1",
                "demo",
                "read_input",
                "demo.read_input",
                "/a.rs",
                1,
                "reads from stdin",
                "let x = read();",
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher.search("parse", None, 100).expect("search");
        // No field contains "parse" → no result returned.
        assert!(results.is_empty(), "expected no results, got {results:?}");
    }

    #[test]
    fn test_bm25f_docstring_match_appears_in_results() {
        // Verify that the fallback CONTAINS scan now checks `docstring`,
        // not just `name`. A symbol whose docstring contains the query
        // should appear in results (ranked below name matches).
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function_with_docstring(
                "f1",
                "demo",
                "compute",
                "demo.compute",
                "/a.rs",
                1,
                "parse the input and return result",
                "",
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher.search("parse", None, 100).expect("search");
        // The symbol's name "compute" doesn't contain "parse", but its
        // docstring does → should be returned.
        assert_eq!(
            results.len(),
            1,
            "expected docstring match, got {results:?}"
        );
        assert_eq!(results[0].name, "compute");
        assert!(results[0].score > 0.0, "expected non-zero score");
    }

    #[test]
    fn test_bm25f_content_match_appears_in_results() {
        // Verify that the fallback CONTAINS scan now checks `content`,
        // not just `name`. A symbol whose content contains the query should
        // appear in results.
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function_with_docstring(
                "f1",
                "demo",
                "compute",
                "demo.compute",
                "/a.rs",
                1,
                "",
                "let parsed = parse_input();",
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher.search("parse", None, 100).expect("search");
        assert_eq!(results.len(), 1, "expected content match, got {results:?}");
        assert_eq!(results[0].name, "compute");
        assert!(results[0].score > 0.0, "expected non-zero score");
    }

    #[test]
    fn test_bm25f_qualified_name_match_appears_in_results() {
        // Verify that the fallback CONTAINS scan now checks `qualifiedName`.
        let repo = fresh_repo();
        repo.save_nodes(
            &[sample_function_with_docstring(
                "f1",
                "demo",
                "compute",
                "demo.parse_helper",
                "/a.rs",
                1,
                "",
                "",
            )],
            NodeLabel::Function,
        )
        .expect("save_nodes");
        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher.search("parse", None, 100).expect("search");
        assert_eq!(
            results.len(),
            1,
            "expected qualifiedName match, got {results:?}"
        );
        assert_eq!(results[0].name, "compute");
    }

    #[test]
    fn test_bm25f_label_without_docstring_content_still_works() {
        // Tables without `docstring`/`content` columns (e.g. Module,
        // Namespace, Variable) must still be scannable without query errors.
        // The fallback should only check `name` and `qualifiedName` for
        // these tables, returning NULL for docstring/content.
        let repo = fresh_repo();
        let module = Node::builder(NodeLabel::Module, "parser", "demo.parser")
            .id("m1")
            .project("demo")
            .file_path("/a.rs")
            .start_line(1)
            .language(Language::Rust)
            .build();
        repo.save_nodes(&[module], NodeLabel::Module)
            .expect("save_nodes");
        let searcher = FullTextSearcher::new(repo.connection());
        let results = searcher.search("parse", None, 100).expect("search");
        // Module name "parser" contains "parse" → returned.
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "parser");
        assert_eq!(results[0].label, "Module");
    }
}
