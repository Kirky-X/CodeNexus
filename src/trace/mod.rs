//! Tracing engine.
//!
//! Performs BFS traversal over `Calls`/`FfiCalls` (call graph) and
//! `DataFlows`/`Reads`/`Writes` (data flow) edges with depth limits, plus
//! impact analysis. Exposed via the [`TraceFacade`] (Facade pattern).
