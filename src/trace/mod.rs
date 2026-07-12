// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Tracing engine (PRD §4.2, ADD §3.4).
//!
//! BFS traversal over call graph and data flow edges, plus impact analysis.

pub mod call_graph;
pub mod capability;
pub mod context;
pub mod data_flow;
pub mod error;
pub mod facade;
pub mod graph_loader;
pub mod impact;
pub mod module;
pub mod types;

pub use call_graph::CallGraphTracer;
pub use context::{collect_incoming, collect_outgoing, collect_processes, resolve_start_id};
pub use data_flow::DataFlowTracer;
pub use error::{Result, TraceError};
pub use facade::{PathFilter, TraceCycle, TraceEngine, TraceFacade, TraceType};
pub use impact::{ImpactAnalyzer, ImpactConfig, ImpactNode, ImpactResult, RiskAssessment, RiskFactor, RiskLevel};
pub use module::{TraceConfig, TraceModule};
pub use types::{
    ContextOutput, RelatedNodeOutput, SymbolNodeOutput, TraceEdge, TraceNode, TracePath,
    TraceResult,
};
