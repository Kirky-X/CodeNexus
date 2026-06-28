// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Cypher subset validation via PEG parsing (H12).
//!
//! [`validate_cypher_subset`] checks that a user-supplied Cypher query uses
//! only the supported clause subset before forwarding it to LadybugDB. This
//! gives a clear, explicit error ("unsupported Cypher construct at …") instead
//! of a raw engine parser exception.
//!
//! # Supported clauses
//!
//! `MATCH`, `OPTIONAL MATCH`, `WHERE`, `WITH`, `RETURN`, `ORDER BY`, `LIMIT`,
//! `SKIP`, `DISTINCT`, `UNWIND`, `UNION [ALL]`, `EXISTS`, variable-length
//! paths (`-[r*1..3]->`), and aggregations (`count`, `sum`, `avg`, `min`,
//! `max`, `collect` — any `ident(args)` is accepted as a function call).
//!
//! # Unsupported clauses
//!
//! Write operations (`CREATE`, `DELETE`, `SET`, `MERGE`, `REMOVE`), procedure
//! calls (`CALL`), `FOREACH`, `LOAD CSV`, and any other clause outside the
//! subset above cause a parse failure → [`QueryError::InvalidQuery`].
//!
//! The grammar lives in [`cypher_subset.pest`](cypher_subset.pest).

use pest::Parser;
use pest_derive::Parser;

use super::error::{QueryError, Result};

/// Auto-generated `Rule` enum from the pest grammar.
///
/// Only [`Rule::query`] is used publicly (via [`validate_cypher_subset`]); the
/// other variants exist because `pest_derive` emits them for every rule in the
/// grammar file.
#[derive(Parser)]
#[grammar = "query/cypher_subset.pest"]
pub struct CypherSubsetParser;

/// Validates that `query` uses only the supported Cypher subset (H12).
///
/// Returns `Ok(())` if the query parses against the subset grammar, or
/// `Err(QueryError::InvalidQuery)` with the pest parse error (position +
/// expected tokens) when an unsupported construct is encountered.
///
/// # Errors
///
/// - [`QueryError::InvalidQuery`] if the query is empty or contains an
///   unsupported construct.
///
/// # Examples
///
/// ```
/// use codenexus::query::cypher_subset::validate_cypher_subset;
///
/// assert!(validate_cypher_subset("MATCH (n:Function) RETURN n.name").is_ok());
/// assert!(validate_cypher_subset("CREATE (n:Person {name: 'a'})").is_err());
/// ```
pub fn validate_cypher_subset(query: &str) -> Result<()> {
    if query.trim().is_empty() {
        return Err(QueryError::InvalidQuery(
            "cypher query must not be empty".to_string(),
        ));
    }
    CypherSubsetParser::parse(Rule::query, query)
        .map(|_| ())
        .map_err(|e| QueryError::InvalidQuery(format!("unsupported Cypher construct: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Supported constructs — must parse without error.
    // -----------------------------------------------------------------------

    #[test]
    fn validates_simple_match_return() {
        assert!(validate_cypher_subset("MATCH (n:Function) RETURN n.name").is_ok());
    }

    #[test]
    fn validates_match_with_alias_and_order_by() {
        let q = "MATCH (f:Function) RETURN f.name AS name ORDER BY f.name";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_match_with_limit() {
        assert!(validate_cypher_subset("MATCH (f:Function) RETURN f.name LIMIT 10").is_ok());
    }

    #[test]
    fn validates_match_with_skip_and_limit() {
        assert!(validate_cypher_subset("MATCH (f:Function) RETURN f.name SKIP 5 LIMIT 10").is_ok());
    }

    #[test]
    fn validates_optional_match() {
        assert!(
            validate_cypher_subset(
                "OPTIONAL MATCH (n:Function) RETURN n.name AS name"
            )
            .is_ok()
        );
    }

    #[test]
    fn validates_where_clause() {
        let q = "MATCH (n:Function) WHERE n.startLine > 10 RETURN n.name";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_where_with_and_or() {
        let q = "MATCH (n:Function) WHERE n.startLine > 10 AND n.name = 'main' OR n.isExported = true RETURN n";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_where_with_not() {
        let q = "MATCH (n:Function) WHERE NOT n.name = 'main' RETURN n.name";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_where_with_string_predicates() {
        let q = "MATCH (n:Function) WHERE n.name STARTS WITH 'parse' AND n.name CONTAINS 'file' RETURN n.name";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_where_with_in_list() {
        let q = "MATCH (n:Function) WHERE n.name IN ['parse', 'read', 'write'] RETURN n.name";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_where_with_is_null() {
        let q = "MATCH (n:Function) WHERE n.docstring IS NULL RETURN n.name";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_where_with_is_not_null() {
        let q = "MATCH (n:Function) WHERE n.docstring IS NOT NULL RETURN n.name";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_with_clause() {
        let q = "MATCH (n:Function) WITH n.name AS name RETURN name ORDER BY name LIMIT 5";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_distinct() {
        assert!(
            validate_cypher_subset("MATCH (n:Function) RETURN DISTINCT n.project").is_ok()
        );
    }

    #[test]
    fn validates_return_star() {
        assert!(validate_cypher_subset("MATCH (n:Function) RETURN *").is_ok());
    }

    #[test]
    fn validates_unwind() {
        let q = "UNWIND [1, 2, 3] AS x RETURN x";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_union() {
        let q = "MATCH (n:Function) RETURN n.name UNION MATCH (m:Method) RETURN m.name";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_union_all() {
        let q = "MATCH (n:Function) RETURN n.name UNION ALL MATCH (m:Method) RETURN m.name";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_exists_pattern() {
        let q = "MATCH (n:Function) WHERE EXISTS((n)-[:CALLS]->(m)) RETURN n.name";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_exists_property() {
        let q = "MATCH (n:Function) WHERE EXISTS(n.docstring) RETURN n.name";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_variable_length_path() {
        let q = "MATCH (n)-[:CALLS*1..3]->(m) RETURN n.name, m.name";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_variable_length_unbounded() {
        let q = "MATCH (n)-[:CALLS*]->(m) RETURN n.name";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_aggregation_count() {
        let q = "MATCH (n:Function) RETURN count(n.name) AS cnt";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_aggregation_count_star() {
        let q = "MATCH (n:Function) RETURN count(*) AS cnt";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_aggregation_count_distinct() {
        let q = "MATCH (n:Function) RETURN count(DISTINCT n.project) AS cnt";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_aggregation_sum_avg() {
        let q = "MATCH (n:Function) RETURN sum(n.startLine) AS total, avg(n.startLine) AS mean";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_collect() {
        let q = "MATCH (n:Function) RETURN n.project AS p, collect(n.name) AS names";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_multi_hop_pattern() {
        let q = "MATCH (a:Function)-[:CALLS]->(b:Function)-[:CALLS]->(c:Function) RETURN a.name, c.name";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_node_with_properties_in_pattern() {
        let q = "MATCH (n:Function {name: 'main'}) RETURN n.filePath";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_rel_with_variable() {
        let q = "MATCH (n)-[r:CALLS]->(m) RETURN r.type, n.name, m.name";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_rel_with_multiple_types() {
        let q = "MATCH (n)-[r:CALLS|:IMPORTS]->(m) RETURN n.name";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_case_insensitive_keywords() {
        assert!(validate_cypher_subset("match (n:Function) return n.name").is_ok());
        assert!(validate_cypher_subset("MATCH (n:Function) Where n.name = 'a' Return n").is_ok());
    }

    #[test]
    fn validates_trailing_semicolon() {
        assert!(validate_cypher_subset("MATCH (n:Function) RETURN n.name;").is_ok());
    }

    #[test]
    fn validates_comment() {
        let q = "// find all functions\nMATCH (n:Function) RETURN n.name";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_parenthesized_expression() {
        let q = "MATCH (n:Function) WHERE (n.startLine > 10 OR n.startLine < 5) RETURN n.name";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_arithmetic_expression() {
        let q = "MATCH (n:Function) RETURN n.startLine + 1 AS next, n.endLine - n.startLine AS length";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_string_literal_single_and_double_quotes() {
        let q = "MATCH (n:Function) WHERE n.name = 'parse' OR n.name = \"read\" RETURN n.name";
        assert!(validate_cypher_subset(q).is_ok());
    }

    #[test]
    fn validates_directed_rel_both_directions() {
        assert!(
            validate_cypher_subset("MATCH (n)<-[:CALLS]-(m) RETURN n.name").is_ok()
        );
        assert!(
            validate_cypher_subset("MATCH (n)-[:CALLS]->(m) RETURN n.name").is_ok()
        );
        assert!(
            validate_cypher_subset("MATCH (n)-[:CALLS]-(m) RETURN n.name").is_ok()
        );
        assert!(
            validate_cypher_subset("MATCH (n)<-[:CALLS]->(m) RETURN n.name").is_ok()
        );
    }

    #[test]
    fn validates_order_by_desc() {
        let q = "MATCH (n:Function) RETURN n.name ORDER BY n.startLine DESC";
        assert!(validate_cypher_subset(q).is_ok());
    }

    // -----------------------------------------------------------------------
    // Unsupported constructs — must return InvalidQuery error.
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_create() {
        let err = validate_cypher_subset("CREATE (n:Person {name: 'a'})").expect_err("CREATE");
        assert!(err.is_invalid_query());
        assert!(err.to_string().contains("unsupported"));
    }

    #[test]
    fn rejects_delete() {
        let err = validate_cypher_subset("MATCH (n) DELETE n").expect_err("DELETE");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn rejects_set() {
        let err = validate_cypher_subset("MATCH (n) SET n.x = 1").expect_err("SET");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn rejects_merge() {
        let err = validate_cypher_subset("MERGE (n:Person {name: 'a'})").expect_err("MERGE");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn rejects_remove() {
        let err = validate_cypher_subset("MATCH (n) REMOVE n.x").expect_err("REMOVE");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn rejects_call() {
        let err = validate_cypher_subset("CALL db.labels()").expect_err("CALL");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn rejects_foreach() {
        let q = "FOREACH (n IN [1,2,3] | CREATE (n))";
        let err = validate_cypher_subset(q).expect_err("FOREACH");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn rejects_load_csv() {
        let q = "LOAD CSV WITH HEADERS FROM 'file:///data.csv' AS row RETURN row";
        let err = validate_cypher_subset(q).expect_err("LOAD CSV");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn rejects_empty_query() {
        let err = validate_cypher_subset("").expect_err("empty");
        assert!(err.is_invalid_query());
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn rejects_whitespace_only_query() {
        let err = validate_cypher_subset("   \n\t  ").expect_err("whitespace");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn rejects_garbage_input() {
        let err = validate_cypher_subset("this is not cypher").expect_err("garbage");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn rejects_keyword_prefix_identifier_in_clause_position() {
        // `MATCHING` is not a supported clause keyword; the parse should fail.
        let err = validate_cypher_subset("MATCHING (n) RETURN n").expect_err("MATCHING");
        assert!(err.is_invalid_query());
    }

    #[test]
    fn rejects_unterminated_string() {
        let err = validate_cypher_subset("MATCH (n) WHERE n.name = 'unterminated RETURN n")
            .expect_err("unterminated string");
        assert!(err.is_invalid_query());
    }
}
