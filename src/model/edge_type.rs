//! Edge type enum representing the 14 relation types in the CodeNexus graph (DDD §7.2).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The 14 edge type variants defined in DDD §7.2.
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
}

impl EdgeType {
    /// Returns all variants in declaration order.
    #[must_use]
    pub const fn all() -> [EdgeType; 14] {
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
            EdgeType::Calls => (0.90, 1.0),
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
            other => Err(format!("unknown EdgeType: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_fourteen_variants() {
        assert_eq!(EdgeType::all().len(), 14);
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
        assert_eq!("DATAFLOWS".parse::<EdgeType>().unwrap(), EdgeType::DataFlows);
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
        assert_eq!(serde_json::to_string(&EdgeType::Calls).unwrap(), "\"Calls\"");
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
        assert_eq!(EdgeType::Calls.confidence_range(), (0.90, 1.0));
        assert_eq!(EdgeType::FfiCalls.confidence_range(), (0.70, 0.85));
        assert_eq!(EdgeType::DataFlows.confidence_range(), (0.80, 0.90));
        assert_eq!(EdgeType::Reads.confidence_range(), (0.70, 0.80));
        assert_eq!(EdgeType::Writes.confidence_range(), (0.70, 0.80));
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
            assert!(
                min <= max,
                "{edge}: min {min} > max {max}"
            );
        }
    }
}
