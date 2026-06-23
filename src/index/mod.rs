//! Indexing pipeline (Facade pattern).
//!
//! Orchestrates discover → parse → resolve → storage, computes SHA-256 file
//! hashes for incremental indexing (ADR-009), and exposes [`IndexFacade`] as
//! the single entry point for the indexing workflow.
