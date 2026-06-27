// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Node label enum representing the 44 node types in the CodeNexus graph
//! (DDD §7.1 base 20 + Interface + H1 extension 23).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The 44 node label variants.
///
/// Each variant corresponds to a node table in the LadybugDB schema and is
/// used as the `label` field on [`crate::model::Node`].
///
/// # Groups
///
/// - **Structural (1-4)**: Project, Folder, File, Module
/// - **Type definitions (5-9, 22-26)**: Class, Struct, Enum, Trait, Impl,
///   Union, Variant, Field, Record, Typedef
/// - **Callables (10-11, 27)**: Function, Method, Constructor
/// - **Variables (12-16, 28)**: Variable, GlobalVar, Parameter, Const,
///   Static, Property
/// - **Meta (17-21)**: Macro, TypeAlias, Namespace, Interface, Delegate
/// - **Annotations/templates (29-30)**: Annotation, Template
/// - **Runtime/architecture (31-37)**: Event, Handler, Middleware, Service,
///   Endpoint, Route, Process
/// - **Data/infra (38-39)**: Database, Config
/// - **Quality/docs (40-41)**: Test, Section
/// - **Community/extension (42-44)**: Community, Tool, Embedding
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeLabel {
    // --- Structural (1-4) ---
    Project,
    Folder,
    File,
    Module,
    // --- Type definitions (5-9) ---
    Class,
    Struct,
    Enum,
    Trait,
    Impl,
    // --- Callables (10-11) ---
    Function,
    Method,
    // --- Variables (12-16) ---
    Variable,
    GlobalVar,
    Parameter,
    Const,
    Static,
    // --- Meta (17-21) ---
    Macro,
    TypeAlias,
    Typedef,
    Namespace,
    Interface,
    // --- H1 extension: Type definitions (22-26) ---
    Constructor,
    Property,
    Record,
    Delegate,
    Annotation,
    // --- H1 extension: Templates (27) ---
    Template,
    // --- H1 extension: Union/Variant/Field (28-30) ---
    Union,
    Variant,
    Field,
    // --- H1 extension: Runtime/architecture (31-37) ---
    Event,
    Handler,
    Middleware,
    Service,
    Endpoint,
    Route,
    Process,
    // --- H1 extension: Data/infra (38-39) ---
    Database,
    Config,
    // --- H1 extension: Quality/docs (40-41) ---
    Test,
    Section,
    // --- H1 extension: Community/extension (42-44) ---
    Community,
    Tool,
    Embedding,
}

impl NodeLabel {
    /// Returns all variants in declaration order.
    #[must_use]
    pub const fn all() -> [NodeLabel; 44] {
        [
            // Structural
            NodeLabel::Project,
            NodeLabel::Folder,
            NodeLabel::File,
            NodeLabel::Module,
            // Type definitions
            NodeLabel::Class,
            NodeLabel::Struct,
            NodeLabel::Enum,
            NodeLabel::Trait,
            NodeLabel::Impl,
            // Callables
            NodeLabel::Function,
            NodeLabel::Method,
            // Variables
            NodeLabel::Variable,
            NodeLabel::GlobalVar,
            NodeLabel::Parameter,
            NodeLabel::Const,
            NodeLabel::Static,
            // Meta
            NodeLabel::Macro,
            NodeLabel::TypeAlias,
            NodeLabel::Typedef,
            NodeLabel::Namespace,
            NodeLabel::Interface,
            // H1 extension: Type definitions
            NodeLabel::Constructor,
            NodeLabel::Property,
            NodeLabel::Record,
            NodeLabel::Delegate,
            NodeLabel::Annotation,
            // H1 extension: Templates
            NodeLabel::Template,
            // H1 extension: Union/Variant/Field
            NodeLabel::Union,
            NodeLabel::Variant,
            NodeLabel::Field,
            // H1 extension: Runtime/architecture
            NodeLabel::Event,
            NodeLabel::Handler,
            NodeLabel::Middleware,
            NodeLabel::Service,
            NodeLabel::Endpoint,
            NodeLabel::Route,
            NodeLabel::Process,
            // H1 extension: Data/infra
            NodeLabel::Database,
            NodeLabel::Config,
            // H1 extension: Quality/docs
            NodeLabel::Test,
            NodeLabel::Section,
            // H1 extension: Community/extension
            NodeLabel::Community,
            NodeLabel::Tool,
            NodeLabel::Embedding,
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
            NodeLabel::Interface => "Interface",
            NodeLabel::Constructor => "Constructor",
            NodeLabel::Property => "Property",
            NodeLabel::Record => "Record",
            NodeLabel::Delegate => "Delegate",
            NodeLabel::Annotation => "Annotation",
            NodeLabel::Template => "Template",
            NodeLabel::Union => "Union",
            NodeLabel::Variant => "Variant",
            NodeLabel::Field => "Field",
            NodeLabel::Event => "Event",
            NodeLabel::Handler => "Handler",
            NodeLabel::Middleware => "Middleware",
            NodeLabel::Service => "Service",
            NodeLabel::Endpoint => "Endpoint",
            NodeLabel::Route => "Route",
            NodeLabel::Process => "Process",
            NodeLabel::Database => "Database",
            NodeLabel::Config => "Config",
            NodeLabel::Test => "Test",
            NodeLabel::Section => "Section",
            NodeLabel::Community => "Community",
            NodeLabel::Tool => "Tool",
            NodeLabel::Embedding => "Embedding",
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
            "interface" => Ok(NodeLabel::Interface),
            "constructor" => Ok(NodeLabel::Constructor),
            "property" => Ok(NodeLabel::Property),
            "record" => Ok(NodeLabel::Record),
            "delegate" => Ok(NodeLabel::Delegate),
            "annotation" => Ok(NodeLabel::Annotation),
            "template" => Ok(NodeLabel::Template),
            "union" => Ok(NodeLabel::Union),
            "variant" => Ok(NodeLabel::Variant),
            "field" => Ok(NodeLabel::Field),
            "event" => Ok(NodeLabel::Event),
            "handler" => Ok(NodeLabel::Handler),
            "middleware" => Ok(NodeLabel::Middleware),
            "service" => Ok(NodeLabel::Service),
            "endpoint" => Ok(NodeLabel::Endpoint),
            "route" => Ok(NodeLabel::Route),
            "process" => Ok(NodeLabel::Process),
            "database" => Ok(NodeLabel::Database),
            "config" => Ok(NodeLabel::Config),
            "test" => Ok(NodeLabel::Test),
            "section" => Ok(NodeLabel::Section),
            "community" => Ok(NodeLabel::Community),
            "tool" => Ok(NodeLabel::Tool),
            "embedding" => Ok(NodeLabel::Embedding),
            other => Err(format!("unknown NodeLabel: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_forty_four_variants() {
        assert_eq!(NodeLabel::all().len(), 44);
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
        assert_eq!(NodeLabel::Interface.to_string(), "Interface");
        // H1 extension
        assert_eq!(NodeLabel::Constructor.to_string(), "Constructor");
        assert_eq!(NodeLabel::Property.to_string(), "Property");
        assert_eq!(NodeLabel::Record.to_string(), "Record");
        assert_eq!(NodeLabel::Delegate.to_string(), "Delegate");
        assert_eq!(NodeLabel::Annotation.to_string(), "Annotation");
        assert_eq!(NodeLabel::Template.to_string(), "Template");
        assert_eq!(NodeLabel::Union.to_string(), "Union");
        assert_eq!(NodeLabel::Variant.to_string(), "Variant");
        assert_eq!(NodeLabel::Field.to_string(), "Field");
        assert_eq!(NodeLabel::Event.to_string(), "Event");
        assert_eq!(NodeLabel::Handler.to_string(), "Handler");
        assert_eq!(NodeLabel::Middleware.to_string(), "Middleware");
        assert_eq!(NodeLabel::Service.to_string(), "Service");
        assert_eq!(NodeLabel::Endpoint.to_string(), "Endpoint");
        assert_eq!(NodeLabel::Route.to_string(), "Route");
        assert_eq!(NodeLabel::Process.to_string(), "Process");
        assert_eq!(NodeLabel::Database.to_string(), "Database");
        assert_eq!(NodeLabel::Config.to_string(), "Config");
        assert_eq!(NodeLabel::Test.to_string(), "Test");
        assert_eq!(NodeLabel::Section.to_string(), "Section");
        assert_eq!(NodeLabel::Community.to_string(), "Community");
        assert_eq!(NodeLabel::Tool.to_string(), "Tool");
        assert_eq!(NodeLabel::Embedding.to_string(), "Embedding");
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
        assert_eq!(
            "function".parse::<NodeLabel>().unwrap(),
            NodeLabel::Function
        );
        assert_eq!(
            "globalvar".parse::<NodeLabel>().unwrap(),
            NodeLabel::GlobalVar
        );
        assert_eq!(
            "typealias".parse::<NodeLabel>().unwrap(),
            NodeLabel::TypeAlias
        );
        assert_eq!("typedef".parse::<NodeLabel>().unwrap(), NodeLabel::Typedef);
        assert_eq!(
            "namespace".parse::<NodeLabel>().unwrap(),
            NodeLabel::Namespace
        );
    }

    #[test]
    fn from_str_parses_uppercase() {
        assert_eq!("PROJECT".parse::<NodeLabel>().unwrap(), NodeLabel::Project);
        assert_eq!(
            "FUNCTION".parse::<NodeLabel>().unwrap(),
            NodeLabel::Function
        );
        assert_eq!(
            "GLOBALVAR".parse::<NodeLabel>().unwrap(),
            NodeLabel::GlobalVar
        );
    }

    #[test]
    fn from_str_parses_mixed_case() {
        assert_eq!("Project".parse::<NodeLabel>().unwrap(), NodeLabel::Project);
        assert_eq!(
            "Function".parse::<NodeLabel>().unwrap(),
            NodeLabel::Function
        );
        assert_eq!(
            "FuNcTiOn".parse::<NodeLabel>().unwrap(),
            NodeLabel::Function
        );
        assert_eq!(
            "GlobalVar".parse::<NodeLabel>().unwrap(),
            NodeLabel::GlobalVar
        );
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
        assert_eq!(
            serde_json::to_string(&NodeLabel::Function).unwrap(),
            "\"Function\""
        );
        assert_eq!(
            serde_json::to_string(&NodeLabel::GlobalVar).unwrap(),
            "\"GlobalVar\""
        );
        assert_eq!(
            serde_json::to_string(&NodeLabel::TypeAlias).unwrap(),
            "\"TypeAlias\""
        );
    }

    #[test]
    fn is_copy() {
        let label = NodeLabel::Function;
        let copied = label;
        assert_eq!(label, copied);
    }
}
