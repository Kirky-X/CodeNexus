//! Domain model entities: nodes, edges, languages, and the in-memory graph.
//!
//! Implements the Builder pattern for [`Node`]/[`Edge`] construction and the
//! Repository-friendly value types described in DDD §4-5 and ADD §3.4.

pub mod edge;
pub mod edge_type;
pub mod flow_type;
pub mod graph;
pub mod language;
pub mod node;
pub mod node_label;

pub use edge::{Edge, EdgeBuilder};
pub use edge_type::EdgeType;
pub use flow_type::FlowType;
pub use graph::Graph;
pub use language::Language;
pub use node::{
    new_file_id, new_func_id, new_node_id, new_node_id_with_prefix, new_project_id, Node,
    NodeBuilder, NodeId,
};
pub use node_label::NodeLabel;
