//! Data flow type enum (DDD §7.4).

use std::fmt;

use serde::{Deserialize, Serialize};

/// The 4 data flow types defined in DDD §7.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FlowType {
    ArgPass,
    ReturnAssign,
    AssignFrom,
    AssignTo,
}

impl FlowType {
    /// Returns all variants in declaration order.
    #[must_use]
    pub const fn all() -> [FlowType; 4] {
        [
            FlowType::ArgPass,
            FlowType::ReturnAssign,
            FlowType::AssignFrom,
            FlowType::AssignTo,
        ]
    }
}

impl fmt::Display for FlowType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlowType::ArgPass => f.write_str("ArgPass"),
            FlowType::ReturnAssign => f.write_str("ReturnAssign"),
            FlowType::AssignFrom => f.write_str("AssignFrom"),
            FlowType::AssignTo => f.write_str("AssignTo"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_four_variants() {
        assert_eq!(FlowType::all().len(), 4);
    }

    #[test]
    fn display_outputs_variant_name() {
        assert_eq!(FlowType::ArgPass.to_string(), "ArgPass");
        assert_eq!(FlowType::ReturnAssign.to_string(), "ReturnAssign");
        assert_eq!(FlowType::AssignFrom.to_string(), "AssignFrom");
        assert_eq!(FlowType::AssignTo.to_string(), "AssignTo");
    }

    #[test]
    fn serde_roundtrip() {
        for flow in FlowType::all() {
            let json = serde_json::to_string(&flow).unwrap();
            let parsed: FlowType = serde_json::from_str(&json).unwrap();
            assert_eq!(flow, parsed);
        }
    }

    #[test]
    fn serde_serializes_as_variant_name() {
        assert_eq!(serde_json::to_string(&FlowType::ArgPass).unwrap(), "\"ArgPass\"");
        assert_eq!(
            serde_json::to_string(&FlowType::ReturnAssign).unwrap(),
            "\"ReturnAssign\""
        );
    }

    #[test]
    fn is_copy() {
        let flow = FlowType::ArgPass;
        let copied = flow;
        assert_eq!(flow, copied);
    }

    #[test]
    fn equality() {
        assert_eq!(FlowType::ArgPass, FlowType::ArgPass);
        assert_ne!(FlowType::ArgPass, FlowType::ReturnAssign);
        assert_ne!(FlowType::AssignFrom, FlowType::AssignTo);
    }
}
