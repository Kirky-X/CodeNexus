// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Edge type enum representing the 30 relation types in the CodeNexus graph (DDD §7.2).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The 30 edge type variants defined in DDD §7.2 (14 original + 10 added in
/// T9 H1 unified graph schema + 6 added for analysis).
///
/// Each variant maps to an UPPERCASE DDL type string used in the LadybugDB
/// `CodeRelation` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeType {
    Contains,
    Defines,
    MemberOf,
    Calls,
    FfiCalls,
    DataFlows,
    Reads,
    Writes,
    Implements,
    Extends,
    UsesType,
    References,
    Imports,
    Includes,
    // --- T9 H1: 10 new edge types for richer graph semantics ---
    /// Class/struct/trait owns a method (structural, explicit in syntax).
    HasMethod,
    /// Class/struct/trait owns a property/field (structural).
    HasProperty,
    /// Function/method accesses a variable or field (inferred from usage).
    Accesses,
    /// Method overrides a parent method (OOP, type-system resolved).
    MethodOverrides,
    /// Method implements an interface/trait method (type-system resolved).
    MethodImplements,
    /// Function/handler is a step in a process (structural).
    StepInProcess,
    /// Handler processes a route/endpoint (structural).
    HandlesRoute,
    /// Function fetches data from a database/service (inferred).
    Fetches,
    /// Handler processes a tool invocation (structural).
    HandlesTool,
    /// Function is the entry point of a process/service (structural).
    EntryPointOf,
    // --- Feature-depth-enhancement: 6 new edge types for analysis ---
    /// Symbol is used by another symbol (inferred from usage).
    Usage,
    /// Test function tests a target symbol (structural).
    Tests,
    /// Function makes an HTTP call to a route/endpoint (inferred).
    HttpCalls,
    /// Function performs an async call (inferred).
    AsyncCalls,
    /// Function emits an event/message (inferred).
    Emits,
    /// Function listens on an event/message (inferred).
    ListensOn,
}

impl EdgeType {
    /// Returns all variants in declaration order.
    #[must_use]
    pub const fn all() -> [EdgeType; 30] {
        [
            EdgeType::Contains,
            EdgeType::Defines,
            EdgeType::MemberOf,
            EdgeType::Calls,
            EdgeType::FfiCalls,
            EdgeType::DataFlows,
            EdgeType::Reads,
            EdgeType::Writes,
            EdgeType::Implements,
            EdgeType::Extends,
            EdgeType::UsesType,
            EdgeType::References,
            EdgeType::Imports,
            EdgeType::Includes,
            EdgeType::HasMethod,
            EdgeType::HasProperty,
            EdgeType::Accesses,
            EdgeType::MethodOverrides,
            EdgeType::MethodImplements,
            EdgeType::StepInProcess,
            EdgeType::HandlesRoute,
            EdgeType::Fetches,
            EdgeType::HandlesTool,
            EdgeType::EntryPointOf,
            EdgeType::Usage,
            EdgeType::Tests,
            EdgeType::HttpCalls,
            EdgeType::AsyncCalls,
            EdgeType::Emits,
            EdgeType::ListensOn,
        ]
    }

    /// Returns the DDL type string (e.g. `"FFI_CALLS"`, `"USES_TYPE"`).
    #[must_use]
    pub fn as_db_type(self) -> &'static str {
        match self {
            EdgeType::Contains => "CONTAINS",
            EdgeType::Defines => "DEFINES",
            EdgeType::MemberOf => "MEMBER_OF",
            EdgeType::Calls => "CALLS",
            EdgeType::FfiCalls => "FFI_CALLS",
            EdgeType::DataFlows => "DATAFLOWS",
            EdgeType::Reads => "READS",
            EdgeType::Writes => "WRITES",
            EdgeType::Implements => "IMPLEMENTS",
            EdgeType::Extends => "EXTENDS",
            EdgeType::UsesType => "USES_TYPE",
            EdgeType::References => "REFERENCES",
            EdgeType::Imports => "IMPORTS",
            EdgeType::Includes => "INCLUDES",
            EdgeType::HasMethod => "HAS_METHOD",
            EdgeType::HasProperty => "HAS_PROPERTY",
            EdgeType::Accesses => "ACCESSES",
            EdgeType::MethodOverrides => "METHOD_OVERRIDES",
            EdgeType::MethodImplements => "METHOD_IMPLEMENTS",
            EdgeType::StepInProcess => "STEP_IN_PROCESS",
            EdgeType::HandlesRoute => "HANDLES_ROUTE",
            EdgeType::Fetches => "FETCHES",
            EdgeType::HandlesTool => "HANDLES_TOOL",
            EdgeType::EntryPointOf => "ENTRY_POINT_OF",
            // Feature-depth-enhancement new edge types
            EdgeType::Usage => "USAGE",
            EdgeType::Tests => "TESTS",
            EdgeType::HttpCalls => "HTTP_CALLS",
            EdgeType::AsyncCalls => "ASYNC_CALLS",
            EdgeType::Emits => "EMITS",
            EdgeType::ListensOn => "LISTENS_ON",
        }
    }

    /// Returns the default confidence range `(min, max)` for this edge type.
    ///
    /// Structural edges derived directly from syntax (e.g. `Contains`,
    /// `Defines`, `Imports`) carry the highest confidence. Edges requiring
    /// cross-language or data-flow inference (e.g. `FfiCalls`, `Reads`,
    /// `Writes`) carry lower confidence. Resolver implementations may pick any
    /// point within this range depending on match strength.
    #[must_use]
    pub fn confidence_range(&self) -> (f32, f32) {
        match self {
            // Structural / syntactic edges — explicit in source.
            EdgeType::Contains => (0.95, 1.0),
            EdgeType::Defines => (0.95, 1.0),
            EdgeType::MemberOf => (0.95, 1.0),
            EdgeType::Imports => (0.95, 1.0),
            EdgeType::Includes => (0.95, 1.0),
            // Type-system edges — resolved with high certainty.
            EdgeType::Implements => (0.90, 1.0),
            EdgeType::Extends => (0.90, 1.0),
            // Call edges — same-language resolution (BR-TRACE-007).
            EdgeType::Calls => (0.80, 0.95),
            // Type / reference usage — requires symbol resolution.
            EdgeType::UsesType => (0.80, 0.90),
            EdgeType::References => (0.75, 0.85),
            // Data flow edges — inferred from assignments and arg passing.
            EdgeType::DataFlows => (0.80, 0.90),
            // Cross-language FFI calls — name and/or signature matching.
            EdgeType::FfiCalls => (0.70, 0.85),
            // Variable read / write access — inferred from usage.
            EdgeType::Reads => (0.70, 0.80),
            EdgeType::Writes => (0.70, 0.80),
            // --- T9 H1: 10 new edge types ---
            // Structural ownership — explicit in syntax.
            EdgeType::HasMethod => (0.95, 1.0),
            EdgeType::HasProperty => (0.95, 1.0),
            // Variable/field access — inferred from usage (like Reads/Writes).
            EdgeType::Accesses => (0.70, 0.80),
            // Type-system edges — resolved with high certainty.
            EdgeType::MethodOverrides => (0.90, 1.0),
            EdgeType::MethodImplements => (0.90, 1.0),
            // Process / handler structural edges — explicit in source.
            EdgeType::StepInProcess => (0.95, 1.0),
            EdgeType::HandlesRoute => (0.90, 1.0),
            EdgeType::HandlesTool => (0.90, 1.0),
            EdgeType::EntryPointOf => (0.95, 1.0),
            // Data fetch — inferred from call patterns.
            EdgeType::Fetches => (0.75, 0.85),
            // Feature-depth-enhancement new edge types
            // Symbol usage — requires symbol resolution (like References).
            EdgeType::Usage => (0.75, 0.85),
            // Test coverage — structural relationship.
            EdgeType::Tests => (0.95, 1.0),
            // HTTP calls — inferred from call patterns.
            EdgeType::HttpCalls => (0.80, 0.90),
            // Async calls — inferred from syntax.
            EdgeType::AsyncCalls => (0.80, 0.90),
            // Event emission — inferred from call patterns.
            EdgeType::Emits => (0.75, 0.85),
            // Event subscription — inferred from call patterns.
            EdgeType::ListensOn => (0.75, 0.85),
        }
    }
}

impl fmt::Display for EdgeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_db_type())
    }
}

impl FromStr for EdgeType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "CONTAINS" => Ok(EdgeType::Contains),
            "DEFINES" => Ok(EdgeType::Defines),
            "MEMBER_OF" => Ok(EdgeType::MemberOf),
            "CALLS" => Ok(EdgeType::Calls),
            "FFI_CALLS" => Ok(EdgeType::FfiCalls),
            "DATAFLOWS" => Ok(EdgeType::DataFlows),
            "READS" => Ok(EdgeType::Reads),
            "WRITES" => Ok(EdgeType::Writes),
            "IMPLEMENTS" => Ok(EdgeType::Implements),
            "EXTENDS" => Ok(EdgeType::Extends),
            "USES_TYPE" => Ok(EdgeType::UsesType),
            "REFERENCES" => Ok(EdgeType::References),
            "IMPORTS" => Ok(EdgeType::Imports),
            "INCLUDES" => Ok(EdgeType::Includes),
            "HAS_METHOD" => Ok(EdgeType::HasMethod),
            "HAS_PROPERTY" => Ok(EdgeType::HasProperty),
            "ACCESSES" => Ok(EdgeType::Accesses),
            "METHOD_OVERRIDES" => Ok(EdgeType::MethodOverrides),
            "METHOD_IMPLEMENTS" => Ok(EdgeType::MethodImplements),
            "STEP_IN_PROCESS" => Ok(EdgeType::StepInProcess),
            "HANDLES_ROUTE" => Ok(EdgeType::HandlesRoute),
            "FETCHES" => Ok(EdgeType::Fetches),
            "HANDLES_TOOL" => Ok(EdgeType::HandlesTool),
            "ENTRY_POINT_OF" => Ok(EdgeType::EntryPointOf),
            "USAGE" => Ok(EdgeType::Usage),
            "TESTS" => Ok(EdgeType::Tests),
            "HTTP_CALLS" => Ok(EdgeType::HttpCalls),
            "ASYNC_CALLS" => Ok(EdgeType::AsyncCalls),
            "EMITS" => Ok(EdgeType::Emits),
            "LISTENS_ON" => Ok(EdgeType::ListensOn),
            other => Err(format!("unknown EdgeType: {other}")),
        }
    }
}

/// Parses a comma-separated list of EdgeType DDL names (case-insensitive).
///
/// Returns the parsed list, or `default_types` if `s` is empty, equals
/// `"all"` (case-insensitive), or contains no valid EdgeType names.
///
/// Each segment is trimmed of surrounding whitespace before parsing, so
/// `" CALLS , USAGE "` is equivalent to `"CALLS,USAGE"`. Invalid segments
/// are silently skipped.
#[must_use]
pub fn parse_edge_type_list(s: &str, default_types: &[EdgeType]) -> Vec<EdgeType> {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("all") {
        return default_types.to_vec();
    }
    let parsed: Vec<EdgeType> = trimmed
        .split(',')
        .filter_map(|part| {
            let t = part.trim();
            if t.is_empty() {
                None
            } else {
                t.to_ascii_uppercase().parse::<EdgeType>().ok()
            }
        })
        .collect();
    if parsed.is_empty() {
        default_types.to_vec()
    } else {
        parsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_thirty_variants() {
        assert_eq!(EdgeType::all().len(), 30);
    }

    #[test]
    fn new_variants_map_to_correct_ddl_strings() {
        assert_eq!(EdgeType::Usage.as_db_type(), "USAGE");
        assert_eq!(EdgeType::Tests.as_db_type(), "TESTS");
        assert_eq!(EdgeType::HttpCalls.as_db_type(), "HTTP_CALLS");
        assert_eq!(EdgeType::AsyncCalls.as_db_type(), "ASYNC_CALLS");
        assert_eq!(EdgeType::Emits.as_db_type(), "EMITS");
        assert_eq!(EdgeType::ListensOn.as_db_type(), "LISTENS_ON");
    }

    #[test]
    fn new_variants_roundtrip_via_from_str() {
        for edge in [
            EdgeType::Usage,
            EdgeType::Tests,
            EdgeType::HttpCalls,
            EdgeType::AsyncCalls,
            EdgeType::Emits,
            EdgeType::ListensOn,
        ] {
            let s = edge.to_string();
            let parsed: EdgeType = s.parse().unwrap();
            assert_eq!(edge, parsed, "roundtrip failed for {s}");
        }
    }

    #[test]
    fn display_outputs_ddl_type_string() {
        assert_eq!(EdgeType::Contains.to_string(), "CONTAINS");
        assert_eq!(EdgeType::Defines.to_string(), "DEFINES");
        assert_eq!(EdgeType::MemberOf.to_string(), "MEMBER_OF");
        assert_eq!(EdgeType::Calls.to_string(), "CALLS");
        assert_eq!(EdgeType::FfiCalls.to_string(), "FFI_CALLS");
        assert_eq!(EdgeType::DataFlows.to_string(), "DATAFLOWS");
        assert_eq!(EdgeType::Reads.to_string(), "READS");
        assert_eq!(EdgeType::Writes.to_string(), "WRITES");
        assert_eq!(EdgeType::Implements.to_string(), "IMPLEMENTS");
        assert_eq!(EdgeType::Extends.to_string(), "EXTENDS");
        assert_eq!(EdgeType::UsesType.to_string(), "USES_TYPE");
        assert_eq!(EdgeType::References.to_string(), "REFERENCES");
        assert_eq!(EdgeType::Imports.to_string(), "IMPORTS");
        assert_eq!(EdgeType::Includes.to_string(), "INCLUDES");
        assert_eq!(EdgeType::HasMethod.to_string(), "HAS_METHOD");
        assert_eq!(EdgeType::HasProperty.to_string(), "HAS_PROPERTY");
        assert_eq!(EdgeType::Accesses.to_string(), "ACCESSES");
        assert_eq!(EdgeType::MethodOverrides.to_string(), "METHOD_OVERRIDES");
        assert_eq!(EdgeType::MethodImplements.to_string(), "METHOD_IMPLEMENTS");
        assert_eq!(EdgeType::StepInProcess.to_string(), "STEP_IN_PROCESS");
        assert_eq!(EdgeType::HandlesRoute.to_string(), "HANDLES_ROUTE");
        assert_eq!(EdgeType::Fetches.to_string(), "FETCHES");
        assert_eq!(EdgeType::HandlesTool.to_string(), "HANDLES_TOOL");
        assert_eq!(EdgeType::EntryPointOf.to_string(), "ENTRY_POINT_OF");
        // Feature-depth-enhancement new edge types
        assert_eq!(EdgeType::Usage.to_string(), "USAGE");
        assert_eq!(EdgeType::Tests.to_string(), "TESTS");
        assert_eq!(EdgeType::HttpCalls.to_string(), "HTTP_CALLS");
        assert_eq!(EdgeType::AsyncCalls.to_string(), "ASYNC_CALLS");
        assert_eq!(EdgeType::Emits.to_string(), "EMITS");
        assert_eq!(EdgeType::ListensOn.to_string(), "LISTENS_ON");
    }

    #[test]
    fn as_db_type_matches_display() {
        for edge in EdgeType::all() {
            assert_eq!(edge.as_db_type(), edge.to_string());
        }
    }

    #[test]
    fn from_str_parses_ddl_strings() {
        assert_eq!("CONTAINS".parse::<EdgeType>().unwrap(), EdgeType::Contains);
        assert_eq!("MEMBER_OF".parse::<EdgeType>().unwrap(), EdgeType::MemberOf);
        assert_eq!("FFI_CALLS".parse::<EdgeType>().unwrap(), EdgeType::FfiCalls);
        assert_eq!("USES_TYPE".parse::<EdgeType>().unwrap(), EdgeType::UsesType);
        assert_eq!(
            "DATAFLOWS".parse::<EdgeType>().unwrap(),
            EdgeType::DataFlows
        );
    }

    #[test]
    fn from_str_parses_all_variants() {
        for edge in EdgeType::all() {
            let s = edge.to_string();
            let parsed: EdgeType = s.parse().unwrap();
            assert_eq!(edge, parsed, "failed for {s}");
        }
    }

    #[test]
    fn from_str_rejects_lowercase() {
        // FromStr parses the DDL type string (UPPERCASE) exactly.
        assert!("contains".parse::<EdgeType>().is_err());
        assert!("calls".parse::<EdgeType>().is_err());
        assert!("ffi_calls".parse::<EdgeType>().is_err());
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!("unknown".parse::<EdgeType>().is_err());
        assert!("".parse::<EdgeType>().is_err());
        assert!("CALLS ".parse::<EdgeType>().is_err());
        assert!(" CALLS".parse::<EdgeType>().is_err());
    }

    #[test]
    fn from_str_error_message_contains_input() {
        let err = "bogus".parse::<EdgeType>().unwrap_err();
        assert!(err.contains("bogus"));
    }

    #[test]
    fn display_fromstr_roundtrip() {
        for edge in EdgeType::all() {
            let s = edge.to_string();
            let parsed: EdgeType = s.parse().unwrap();
            assert_eq!(edge, parsed);
        }
    }

    #[test]
    fn serde_roundtrip() {
        for edge in EdgeType::all() {
            let json = serde_json::to_string(&edge).unwrap();
            let parsed: EdgeType = serde_json::from_str(&json).unwrap();
            assert_eq!(edge, parsed);
        }
    }

    #[test]
    fn serde_serializes_as_variant_name() {
        assert_eq!(
            serde_json::to_string(&EdgeType::Calls).unwrap(),
            "\"Calls\""
        );
        assert_eq!(
            serde_json::to_string(&EdgeType::FfiCalls).unwrap(),
            "\"FfiCalls\""
        );
        assert_eq!(
            serde_json::to_string(&EdgeType::UsesType).unwrap(),
            "\"UsesType\""
        );
    }

    #[test]
    fn is_copy() {
        let edge = EdgeType::Calls;
        let copied = edge;
        assert_eq!(edge, copied);
    }

    #[test]
    fn confidence_range_returns_expected_ranges() {
        assert_eq!(EdgeType::Calls.confidence_range(), (0.80, 0.95));
        assert_eq!(EdgeType::FfiCalls.confidence_range(), (0.70, 0.85));
        assert_eq!(EdgeType::DataFlows.confidence_range(), (0.80, 0.90));
        assert_eq!(EdgeType::Reads.confidence_range(), (0.70, 0.80));
        assert_eq!(EdgeType::Writes.confidence_range(), (0.70, 0.80));
        // T9 H1 new edge types
        assert_eq!(EdgeType::HasMethod.confidence_range(), (0.95, 1.0));
        assert_eq!(EdgeType::HasProperty.confidence_range(), (0.95, 1.0));
        assert_eq!(EdgeType::Accesses.confidence_range(), (0.70, 0.80));
        assert_eq!(EdgeType::MethodOverrides.confidence_range(), (0.90, 1.0));
        assert_eq!(EdgeType::MethodImplements.confidence_range(), (0.90, 1.0));
        assert_eq!(EdgeType::StepInProcess.confidence_range(), (0.95, 1.0));
        assert_eq!(EdgeType::HandlesRoute.confidence_range(), (0.90, 1.0));
        assert_eq!(EdgeType::Fetches.confidence_range(), (0.75, 0.85));
        assert_eq!(EdgeType::HandlesTool.confidence_range(), (0.90, 1.0));
        assert_eq!(EdgeType::EntryPointOf.confidence_range(), (0.95, 1.0));
        // Feature-depth-enhancement new edge types
        assert_eq!(EdgeType::Usage.confidence_range(), (0.75, 0.85));
        assert_eq!(EdgeType::Tests.confidence_range(), (0.95, 1.0));
        assert_eq!(EdgeType::HttpCalls.confidence_range(), (0.80, 0.90));
        assert_eq!(EdgeType::AsyncCalls.confidence_range(), (0.80, 0.90));
        assert_eq!(EdgeType::Emits.confidence_range(), (0.75, 0.85));
        assert_eq!(EdgeType::ListensOn.confidence_range(), (0.75, 0.85));
    }

    #[test]
    fn calls_confidence_range_includes_project_confidence() {
        // BR-TRACE-007: Calls confidence range is 0.80-0.95.
        // CONFIDENCE_PROJECT = 0.80 should be within range.
        let (min, max) = EdgeType::Calls.confidence_range();
        let confidence_project: f32 = 0.80;
        assert!(
            confidence_project >= min,
            "CONFIDENCE_PROJECT {} < min {}",
            confidence_project,
            min
        );
        assert!(
            confidence_project <= max,
            "CONFIDENCE_PROJECT {} > max {}",
            confidence_project,
            max
        );
    }

    #[test]
    fn confidence_range_all_variants_have_valid_bounds() {
        for edge in EdgeType::all() {
            let (min, max) = edge.confidence_range();
            assert!(
                (0.0..=1.0).contains(&min),
                "{edge}: min {min} out of [0.0, 1.0]"
            );
            assert!(
                (0.0..=1.0).contains(&max),
                "{edge}: max {max} out of [0.0, 1.0]"
            );
            assert!(min <= max, "{edge}: min {min} > max {max}");
        }
    }

    // ===== parse_edge_type_list unit tests =====

    #[test]
    fn parse_edge_type_list_parses_lowercase_names() {
        let defaults = [EdgeType::Calls, EdgeType::Usage];
        let result = parse_edge_type_list("calls,usage", &defaults);
        assert_eq!(result, vec![EdgeType::Calls, EdgeType::Usage]);
    }

    #[test]
    fn parse_edge_type_list_parses_uppercase_names() {
        let defaults = [EdgeType::Calls];
        let result = parse_edge_type_list("CALLS,USAGE", &defaults);
        assert_eq!(result, vec![EdgeType::Calls, EdgeType::Usage]);
    }

    #[test]
    fn parse_edge_type_list_empty_returns_defaults() {
        let defaults = [EdgeType::Calls, EdgeType::Usage, EdgeType::Tests];
        let result = parse_edge_type_list("", &defaults);
        assert_eq!(result, defaults.to_vec());
    }

    #[test]
    fn parse_edge_type_list_all_returns_defaults() {
        let defaults = [EdgeType::Calls, EdgeType::Usage];
        let result = parse_edge_type_list("all", &defaults);
        assert_eq!(result, defaults.to_vec());
    }

    #[test]
    fn parse_edge_type_list_all_is_case_insensitive() {
        let defaults = [EdgeType::Calls];
        let result = parse_edge_type_list("ALL", &defaults);
        assert_eq!(result, defaults.to_vec());
    }

    #[test]
    fn parse_edge_type_list_skips_invalid_segments() {
        let defaults = [EdgeType::Calls];
        let result = parse_edge_type_list("CALLS,INVALID,TESTS", &defaults);
        assert_eq!(result, vec![EdgeType::Calls, EdgeType::Tests]);
    }

    #[test]
    fn parse_edge_type_list_all_invalid_returns_defaults() {
        let defaults = [EdgeType::Calls, EdgeType::Usage];
        let result = parse_edge_type_list("INVALID1,INVALID2", &defaults);
        assert_eq!(result, defaults.to_vec());
    }

    #[test]
    fn parse_edge_type_list_trims_whitespace() {
        let defaults = [EdgeType::Calls];
        let result = parse_edge_type_list("  CALLS ,  USAGE  ", &defaults);
        assert_eq!(result, vec![EdgeType::Calls, EdgeType::Usage]);
    }

    #[test]
    fn parse_edge_type_list_single_type() {
        let defaults = [EdgeType::Calls];
        let result = parse_edge_type_list("HTTP_CALLS", &defaults);
        assert_eq!(result, vec![EdgeType::HttpCalls]);
    }

    #[test]
    fn parse_edge_type_list_whitespace_only_returns_defaults() {
        let defaults = [EdgeType::Calls, EdgeType::Usage];
        let result = parse_edge_type_list("   ", &defaults);
        assert_eq!(result, defaults.to_vec());
    }
}
