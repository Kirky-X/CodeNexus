//! Optional vector embedding subsystem (Strategy pattern).
//!
//! Gated behind the `embed` feature (ADR-004). Provides an HTTP client for
//! OpenAI-compatible embedding APIs, vector storage in LadybugDB, and
//! pluggable search strategies (BM25 / semantic / hybrid).
