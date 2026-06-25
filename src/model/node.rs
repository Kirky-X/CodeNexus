//! Node entity, NodeId type, and UUIDv7 ID generators (ADD §3.4, DDD §5).

use serde::{Deserialize, Serialize};

use super::{Language, NodeLabel};

/// The type used for node identifiers (UUIDv7 strings, DDD uses STRING PK).
pub type NodeId = String;

/// Generates a new UUIDv7 node identifier.
#[must_use]
pub fn new_node_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Generates a project-prefixed node identifier (`proj_<uuid>`).
#[must_use]
pub fn new_project_id() -> String {
    new_node_id_with_prefix("proj")
}

/// Generates a file-prefixed node identifier (`file_<uuid>`).
#[must_use]
pub fn new_file_id() -> String {
    new_node_id_with_prefix("file")
}

/// Generates a function-prefixed node identifier (`func_<uuid>`).
#[must_use]
pub fn new_func_id() -> String {
    new_node_id_with_prefix("func")
}

/// Generates a node identifier with a custom prefix (`<prefix>_<uuid>`).
#[must_use]
pub fn new_node_id_with_prefix(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::now_v7())
}

/// A node in the code knowledge graph (ADD §3.4, DDD §5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// UUIDv7 string identifier.
    pub id: NodeId,
    /// The node label (one of 20 types, DDD §7.1).
    pub label: NodeLabel,
    /// Short display name of the node.
    pub name: String,
    /// Fully qualified name (project.dir.file.entity, ADD §7.1).
    pub qualified_name: String,
    /// Source file path, if applicable.
    pub file_path: Option<String>,
    /// Start line in the source file (1-based).
    pub start_line: Option<u32>,
    /// End line in the source file (1-based, inclusive).
    pub end_line: Option<u32>,
    /// Source language, if applicable.
    pub language: Option<Language>,
    /// Function/method signature, if applicable.
    pub signature: Option<String>,
    /// Return type, if applicable.
    pub return_type: Option<String>,
    /// Docstring/comment, if applicable.
    pub docstring: Option<String>,
    /// Whether the symbol is exported (public API).
    pub is_exported: bool,
    /// Whether the symbol has global scope.
    pub is_global: bool,
    /// Qualified name of the parent symbol.
    pub parent_qn: Option<String>,
    /// JSON object for extra fields.
    pub properties: serde_json::Value,
    /// The project this node belongs to (multi-project isolation, DDD §2.3).
    pub project: String,
}

impl Node {
    /// Creates a [`NodeBuilder`] with the required fields.
    ///
    /// Defaults: `id` auto-generated via [`new_node_id`], `is_exported=false`,
    /// `is_global=false`, `properties=Null`, `project=""`.
    pub fn builder(
        label: NodeLabel,
        name: impl Into<String>,
        qualified_name: impl Into<String>,
    ) -> NodeBuilder {
        NodeBuilder {
            node: Node {
                id: new_node_id(),
                label,
                name: name.into(),
                qualified_name: qualified_name.into(),
                file_path: None,
                start_line: None,
                end_line: None,
                language: None,
                signature: None,
                return_type: None,
                docstring: None,
                is_exported: false,
                is_global: false,
                parent_qn: None,
                properties: serde_json::Value::Null,
                project: String::new(),
            },
        }
    }

    /// Returns the project this node belongs to.
    #[must_use]
    pub fn project(&self) -> &str {
        &self.project
    }
}

/// Builder for [`Node`] using the fluent setter pattern.
#[must_use]
#[derive(Debug, Clone)]
pub struct NodeBuilder {
    node: Node,
}

impl NodeBuilder {
    /// Sets the node id.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.node.id = id.into();
        self
    }

    /// Sets the node label.
    pub fn label(mut self, label: NodeLabel) -> Self {
        self.node.label = label;
        self
    }

    /// Sets the node name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.node.name = name.into();
        self
    }

    /// Sets the qualified name.
    pub fn qualified_name(mut self, qualified_name: impl Into<String>) -> Self {
        self.node.qualified_name = qualified_name.into();
        self
    }

    /// Sets the source file path.
    pub fn file_path(mut self, file_path: impl Into<String>) -> Self {
        self.node.file_path = Some(file_path.into());
        self
    }

    /// Sets the start line (1-based).
    pub fn start_line(mut self, start_line: u32) -> Self {
        self.node.start_line = Some(start_line);
        self
    }

    /// Sets the end line (1-based, inclusive).
    pub fn end_line(mut self, end_line: u32) -> Self {
        self.node.end_line = Some(end_line);
        self
    }

    /// Sets the source language.
    pub fn language(mut self, language: Language) -> Self {
        self.node.language = Some(language);
        self
    }

    /// Sets the function/method signature.
    pub fn signature(mut self, signature: impl Into<String>) -> Self {
        self.node.signature = Some(signature.into());
        self
    }

    /// Sets the return type.
    pub fn return_type(mut self, return_type: impl Into<String>) -> Self {
        self.node.return_type = Some(return_type.into());
        self
    }

    /// Sets the docstring.
    pub fn docstring(mut self, docstring: impl Into<String>) -> Self {
        self.node.docstring = Some(docstring.into());
        self
    }

    /// Sets whether the symbol is exported.
    pub fn is_exported(mut self, is_exported: bool) -> Self {
        self.node.is_exported = is_exported;
        self
    }

    /// Sets whether the symbol has global scope.
    pub fn is_global(mut self, is_global: bool) -> Self {
        self.node.is_global = is_global;
        self
    }

    /// Sets the parent qualified name.
    pub fn parent_qn(mut self, parent_qn: impl Into<String>) -> Self {
        self.node.parent_qn = Some(parent_qn.into());
        self
    }

    /// Sets the extra properties JSON.
    pub fn properties(mut self, properties: serde_json::Value) -> Self {
        self.node.properties = properties;
        self
    }

    /// Sets the project (multi-project isolation, DDD §2.3).
    pub fn project(mut self, project: impl Into<String>) -> Self {
        self.node.project = project.into();
        self
    }

    /// Builds the [`Node`].
    #[must_use]
    pub fn build(self) -> Node {
        self.node
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_node_id_returns_non_empty_uuid() {
        let id = new_node_id();
        assert!(!id.is_empty());
        // UUIDv7 strings are 36 chars (8-4-4-4-12 with hyphens).
        assert_eq!(id.len(), 36);
        assert!(id.parse::<uuid::Uuid>().is_ok());
    }

    #[test]
    fn new_node_id_generates_unique_ids() {
        let a = new_node_id();
        let b = new_node_id();
        assert_ne!(a, b);
    }

    #[test]
    fn new_node_id_generates_v7() {
        let id = new_node_id();
        let uuid = id.parse::<uuid::Uuid>().unwrap();
        assert_eq!(uuid.get_version_num(), 7);
    }

    #[test]
    fn new_project_id_has_prefix() {
        let id = new_project_id();
        assert!(id.starts_with("proj_"));
        assert!(id.len() > "proj_".len());
    }

    #[test]
    fn new_file_id_has_prefix() {
        let id = new_file_id();
        assert!(id.starts_with("file_"));
        assert!(id.len() > "file_".len());
    }

    #[test]
    fn new_func_id_has_prefix() {
        let id = new_func_id();
        assert!(id.starts_with("func_"));
        assert!(id.len() > "func_".len());
    }

    #[test]
    fn new_node_id_with_prefix_uses_custom_prefix() {
        let id = new_node_id_with_prefix("var");
        assert!(id.starts_with("var_"));
        assert!(id.len() > "var_".len());
    }

    #[test]
    fn new_node_id_with_prefix_empty_prefix() {
        let id = new_node_id_with_prefix("");
        assert!(id.starts_with('_'));
    }

    #[test]
    fn prefixed_ids_are_unique() {
        let a = new_project_id();
        let b = new_project_id();
        assert_ne!(a, b);
    }

    #[test]
    fn builder_sets_required_fields() {
        let node = Node::builder(NodeLabel::Function, "foo", "proj.src.foo").build();
        assert_eq!(node.label, NodeLabel::Function);
        assert_eq!(node.name, "foo");
        assert_eq!(node.qualified_name, "proj.src.foo");
    }

    #[test]
    fn builder_defaults() {
        let node = Node::builder(NodeLabel::Function, "foo", "qn").build();
        assert!(!node.id.is_empty());
        assert_eq!(node.label, NodeLabel::Function);
        assert_eq!(node.name, "foo");
        assert_eq!(node.qualified_name, "qn");
        assert_eq!(node.file_path, None);
        assert_eq!(node.start_line, None);
        assert_eq!(node.end_line, None);
        assert_eq!(node.language, None);
        assert_eq!(node.signature, None);
        assert_eq!(node.return_type, None);
        assert_eq!(node.docstring, None);
        assert!(!node.is_exported);
        assert!(!node.is_global);
        assert_eq!(node.parent_qn, None);
        assert_eq!(node.properties, serde_json::Value::Null);
        assert_eq!(node.project, "");
    }

    #[test]
    fn builder_auto_generates_id() {
        let node = Node::builder(NodeLabel::Function, "foo", "qn").build();
        assert!(node.id.parse::<uuid::Uuid>().is_ok());
    }

    #[test]
    fn builder_custom_id() {
        let node = Node::builder(NodeLabel::Function, "foo", "qn")
            .id("custom-id")
            .build();
        assert_eq!(node.id, "custom-id");
    }

    #[test]
    fn builder_fluent_setters() {
        let node = Node::builder(NodeLabel::Method, "bar", "qn.bar")
            .label(NodeLabel::Function)
            .name("baz")
            .qualified_name("qn.baz")
            .file_path("/src/main.rs")
            .start_line(10)
            .end_line(20)
            .language(Language::Rust)
            .signature("fn baz(x: i32) -> i32")
            .return_type("i32")
            .docstring("Does a thing.")
            .is_exported(true)
            .is_global(true)
            .parent_qn("qn")
            .properties(serde_json::json!({"visibility": "public"}))
            .project("my-project")
            .build();

        assert_eq!(node.label, NodeLabel::Function);
        assert_eq!(node.name, "baz");
        assert_eq!(node.qualified_name, "qn.baz");
        assert_eq!(node.file_path.as_deref(), Some("/src/main.rs"));
        assert_eq!(node.start_line, Some(10));
        assert_eq!(node.end_line, Some(20));
        assert_eq!(node.language, Some(Language::Rust));
        assert_eq!(node.signature.as_deref(), Some("fn baz(x: i32) -> i32"));
        assert_eq!(node.return_type.as_deref(), Some("i32"));
        assert_eq!(node.docstring.as_deref(), Some("Does a thing."));
        assert!(node.is_exported);
        assert!(node.is_global);
        assert_eq!(node.parent_qn.as_deref(), Some("qn"));
        assert_eq!(node.properties, serde_json::json!({"visibility": "public"}));
        assert_eq!(node.project, "my-project");
    }

    #[test]
    fn builder_setters_accept_string_and_str() {
        let name = String::from("foo");
        let qn = String::from("qn.foo");
        let node = Node::builder(NodeLabel::Function, name, qn)
            .file_path(String::from("/src/main.rs"))
            .signature(String::from("fn foo()"))
            .build();
        assert_eq!(node.name, "foo");
        assert_eq!(node.qualified_name, "qn.foo");
        assert_eq!(node.file_path.as_deref(), Some("/src/main.rs"));
        assert_eq!(node.signature.as_deref(), Some("fn foo()"));
    }

    #[test]
    fn project_method_returns_project() {
        let node = Node::builder(NodeLabel::Function, "foo", "qn")
            .project("my-project")
            .build();
        assert_eq!(node.project(), "my-project");
    }

    #[test]
    fn project_method_defaults_to_empty() {
        let node = Node::builder(NodeLabel::Function, "foo", "qn").build();
        assert_eq!(node.project(), "");
    }

    #[test]
    fn serde_roundtrip() {
        let node = Node::builder(NodeLabel::Function, "foo", "qn.foo")
            .file_path("/src/main.rs")
            .start_line(1)
            .end_line(10)
            .language(Language::Rust)
            .signature("fn foo()")
            .return_type("()")
            .docstring("docs")
            .is_exported(true)
            .is_global(false)
            .parent_qn("qn")
            .properties(serde_json::json!({"k": "v"}))
            .project("proj")
            .build();

        let json = serde_json::to_string(&node).unwrap();
        let parsed: Node = serde_json::from_str(&json).unwrap();
        assert_eq!(node, parsed);
    }

    #[test]
    fn clone_is_equal() {
        let node = Node::builder(NodeLabel::Function, "foo", "qn")
            .project("p")
            .build();
        let cloned = node.clone();
        assert_eq!(node, cloned);
    }

    #[test]
    fn debug_is_non_empty() {
        let node = Node::builder(NodeLabel::Function, "foo", "qn").build();
        let debug = format!("{node:?}");
        assert!(debug.contains("Node"));
        assert!(debug.contains("foo"));
    }
}
