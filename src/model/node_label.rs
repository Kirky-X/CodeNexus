//! Node label enum representing the 20 node types in the CodeNexus graph (DDD §7.1).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The 20 node label variants defined in DDD §7.1.
///
/// Each variant corresponds to a node table in the LadybugDB schema and is
/// used as the `label` field on [`crate::model::Node`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeLabel {
    Project,
    Folder,
    File,
    Module,
    Class,
    Struct,
    Enum,
    Trait,
    Impl,
    Function,
    Method,
    Variable,
    GlobalVar,
    Parameter,
    Const,
    Static,
    Macro,
    TypeAlias,
    Typedef,
    Namespace,
}

impl NodeLabel {
    /// Returns all variants in declaration order.
    #[must_use]
    pub const fn all() -> [NodeLabel; 20] {
        [
            NodeLabel::Project,
            NodeLabel::Folder,
            NodeLabel::File,
            NodeLabel::Module,
            NodeLabel::Class,
            NodeLabel::Struct,
            NodeLabel::Enum,
            NodeLabel::Trait,
            NodeLabel::Impl,
            NodeLabel::Function,
            NodeLabel::Method,
            NodeLabel::Variable,
            NodeLabel::GlobalVar,
            NodeLabel::Parameter,
            NodeLabel::Const,
            NodeLabel::Static,
            NodeLabel::Macro,
            NodeLabel::TypeAlias,
            NodeLabel::Typedef,
            NodeLabel::Namespace,
        ]
    }

    /// Returns the table name for this label (same as [`Display`], e.g.
    /// `"Function"`, `"GlobalVar"`).
    #[must_use]
    pub fn table_name(self) -> &'static str {
        match self {
            NodeLabel::Project => "Project",
            NodeLabel::Folder => "Folder",
            NodeLabel::File => "File",
            NodeLabel::Module => "Module",
            NodeLabel::Class => "Class",
            NodeLabel::Struct => "Struct",
            NodeLabel::Enum => "Enum",
            NodeLabel::Trait => "Trait",
            NodeLabel::Impl => "Impl",
            NodeLabel::Function => "Function",
            NodeLabel::Method => "Method",
            NodeLabel::Variable => "Variable",
            NodeLabel::GlobalVar => "GlobalVar",
            NodeLabel::Parameter => "Parameter",
            NodeLabel::Const => "Const",
            NodeLabel::Static => "Static",
            NodeLabel::Macro => "Macro",
            NodeLabel::TypeAlias => "TypeAlias",
            NodeLabel::Typedef => "Typedef",
            NodeLabel::Namespace => "Namespace",
        }
    }
}

impl fmt::Display for NodeLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.table_name())
    }
}

impl FromStr for NodeLabel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "project" => Ok(NodeLabel::Project),
            "folder" => Ok(NodeLabel::Folder),
            "file" => Ok(NodeLabel::File),
            "module" => Ok(NodeLabel::Module),
            "class" => Ok(NodeLabel::Class),
            "struct" => Ok(NodeLabel::Struct),
            "enum" => Ok(NodeLabel::Enum),
            "trait" => Ok(NodeLabel::Trait),
            "impl" => Ok(NodeLabel::Impl),
            "function" => Ok(NodeLabel::Function),
            "method" => Ok(NodeLabel::Method),
            "variable" => Ok(NodeLabel::Variable),
            "globalvar" => Ok(NodeLabel::GlobalVar),
            "parameter" => Ok(NodeLabel::Parameter),
            "const" => Ok(NodeLabel::Const),
            "static" => Ok(NodeLabel::Static),
            "macro" => Ok(NodeLabel::Macro),
            "typealias" => Ok(NodeLabel::TypeAlias),
            "typedef" => Ok(NodeLabel::Typedef),
            "namespace" => Ok(NodeLabel::Namespace),
            other => Err(format!("unknown NodeLabel: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_twenty_variants() {
        assert_eq!(NodeLabel::all().len(), 20);
    }

    #[test]
    fn display_outputs_variant_name() {
        assert_eq!(NodeLabel::Project.to_string(), "Project");
        assert_eq!(NodeLabel::Folder.to_string(), "Folder");
        assert_eq!(NodeLabel::File.to_string(), "File");
        assert_eq!(NodeLabel::Module.to_string(), "Module");
        assert_eq!(NodeLabel::Class.to_string(), "Class");
        assert_eq!(NodeLabel::Struct.to_string(), "Struct");
        assert_eq!(NodeLabel::Enum.to_string(), "Enum");
        assert_eq!(NodeLabel::Trait.to_string(), "Trait");
        assert_eq!(NodeLabel::Impl.to_string(), "Impl");
        assert_eq!(NodeLabel::Function.to_string(), "Function");
        assert_eq!(NodeLabel::Method.to_string(), "Method");
        assert_eq!(NodeLabel::Variable.to_string(), "Variable");
        assert_eq!(NodeLabel::GlobalVar.to_string(), "GlobalVar");
        assert_eq!(NodeLabel::Parameter.to_string(), "Parameter");
        assert_eq!(NodeLabel::Const.to_string(), "Const");
        assert_eq!(NodeLabel::Static.to_string(), "Static");
        assert_eq!(NodeLabel::Macro.to_string(), "Macro");
        assert_eq!(NodeLabel::TypeAlias.to_string(), "TypeAlias");
        assert_eq!(NodeLabel::Typedef.to_string(), "Typedef");
        assert_eq!(NodeLabel::Namespace.to_string(), "Namespace");
    }

    #[test]
    fn table_name_matches_display() {
        for label in NodeLabel::all() {
            assert_eq!(label.table_name(), label.to_string());
        }
    }

    #[test]
    fn globalvar_table_name_is_preserved() {
        // DDD §7.1 note: GlobalVar stays "GlobalVar" (not "Global_Var").
        assert_eq!(NodeLabel::GlobalVar.table_name(), "GlobalVar");
    }

    #[test]
    fn from_str_parses_lowercase() {
        assert_eq!("project".parse::<NodeLabel>().unwrap(), NodeLabel::Project);
        assert_eq!("function".parse::<NodeLabel>().unwrap(), NodeLabel::Function);
        assert_eq!("globalvar".parse::<NodeLabel>().unwrap(), NodeLabel::GlobalVar);
        assert_eq!("typealias".parse::<NodeLabel>().unwrap(), NodeLabel::TypeAlias);
        assert_eq!("typedef".parse::<NodeLabel>().unwrap(), NodeLabel::Typedef);
        assert_eq!("namespace".parse::<NodeLabel>().unwrap(), NodeLabel::Namespace);
    }

    #[test]
    fn from_str_parses_uppercase() {
        assert_eq!("PROJECT".parse::<NodeLabel>().unwrap(), NodeLabel::Project);
        assert_eq!("FUNCTION".parse::<NodeLabel>().unwrap(), NodeLabel::Function);
        assert_eq!("GLOBALVAR".parse::<NodeLabel>().unwrap(), NodeLabel::GlobalVar);
    }

    #[test]
    fn from_str_parses_mixed_case() {
        assert_eq!("Project".parse::<NodeLabel>().unwrap(), NodeLabel::Project);
        assert_eq!("Function".parse::<NodeLabel>().unwrap(), NodeLabel::Function);
        assert_eq!("FuNcTiOn".parse::<NodeLabel>().unwrap(), NodeLabel::Function);
        assert_eq!("GlobalVar".parse::<NodeLabel>().unwrap(), NodeLabel::GlobalVar);
    }

    #[test]
    fn from_str_parses_all_variants() {
        for label in NodeLabel::all() {
            let lower = label.to_string().to_ascii_lowercase();
            let parsed: NodeLabel = lower.parse().unwrap();
            assert_eq!(label, parsed, "failed for lowercase {lower}");
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!("unknown".parse::<NodeLabel>().is_err());
        assert!("".parse::<NodeLabel>().is_err());
        assert!("function ".parse::<NodeLabel>().is_err());
        assert!("function_".parse::<NodeLabel>().is_err());
        assert!("global_var".parse::<NodeLabel>().is_err());
        assert!("type_alias".parse::<NodeLabel>().is_err());
    }

    #[test]
    fn from_str_error_message_contains_input() {
        let err = "bogus".parse::<NodeLabel>().unwrap_err();
        assert!(err.contains("bogus"));
    }

    #[test]
    fn display_fromstr_roundtrip() {
        for label in NodeLabel::all() {
            let s = label.to_string();
            let parsed: NodeLabel = s.parse().unwrap();
            assert_eq!(label, parsed);
        }
    }

    #[test]
    fn serde_roundtrip() {
        for label in NodeLabel::all() {
            let json = serde_json::to_string(&label).unwrap();
            let parsed: NodeLabel = serde_json::from_str(&json).unwrap();
            assert_eq!(label, parsed);
        }
    }

    #[test]
    fn serde_serializes_as_variant_name() {
        assert_eq!(serde_json::to_string(&NodeLabel::Function).unwrap(), "\"Function\"");
        assert_eq!(serde_json::to_string(&NodeLabel::GlobalVar).unwrap(), "\"GlobalVar\"");
        assert_eq!(serde_json::to_string(&NodeLabel::TypeAlias).unwrap(), "\"TypeAlias\"");
    }

    #[test]
    fn is_copy() {
        let label = NodeLabel::Function;
        let copied = label;
        assert_eq!(label, copied);
    }
}
