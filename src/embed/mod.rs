// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Optional vector embedding subsystem (Strategy pattern).
//!
//! Gated behind the `embed` feature (ADR-004). Provides two embedding backends:
//!
//! - **Remote HTTP** ([`OpenAIEmbedClient`]): calls an OpenAI-compatible
//!   embedding API via `reqwest`. API keys are read from environment variables
//!   and never persisted (TRD §6.1).
//! - **Local ONNX** ([`LocalEmbedClient`], H10/D7): runs `arctic-embed-xs`
//!   inference locally via `ort` (ONNX Runtime). Works fully offline — no API
//!   key, no network access required. The model file must be present on disk.
//!
//! Backend selection is driven by [`EmbeddingConfig::endpoint`]:
//! `Some(url)` → remote HTTP, `None` → local ONNX. The default is local
//! (offline), so `--embed` works out of the box without any API key.
//!
//! # Architecture
//!
//! - [`client`]: [`EmbedClient`] trait + [`OpenAIEmbedClient`] (reqwest HTTP)
//!   + [`LocalEmbedClient`] (ort ONNX) + [`MockEmbedClient`] (test).
//! - [`storage`]: [`EmbeddingStorage`] stores/retrieves `FLOAT[384]` vectors in
//!   the LadybugDB `Embedding` table (DDD §5.9).
//! - [`search`]: [`SearchStrategy`] trait with [`Bm25Strategy`],
//!   [`SemanticStrategy`], and [`HybridStrategy`] (RRF fusion, AC-SEARCH-002).
//!
//! # Degradation
//!
//! On Windows the LadybugDB VECTOR extension is unavailable (R-003/TR-005);
//! [`search::is_vector_supported`] returns `false` and the search strategy
//! degrades to BM25-only. If the embedding service is unreachable (remote
//! mode) or the model file is missing (local mode), indexing continues
//! without embeddings (SubTask 16.4).
//!
//! # trait-kit integration (Task 2.12)
//!
//! When the `embed` feature is enabled, [`client::EmbedClient`] is the
//! capability trait stored in [`Kit`](crate::kit::Kit) under
//! [`EmbedKey`](crate::kit::EmbedKey). The concrete impl
//! ([`module::EmbedCapability`]) lazily loads the local model or creates a
//! fresh HTTP client per call, depending on [`EmbeddingConfig::endpoint`].

pub mod client;
pub mod config;
pub mod error;
pub mod module;
pub mod search;
pub mod storage;

pub use client::{EmbedClient, LocalEmbedClient, MockEmbedClient, OpenAIEmbedClient};
pub use config::{
    EmbeddingConfig, API_KEY_ENV, DEFAULT_MODEL_PATH, DEFAULT_TOKENIZER_FILENAME,
    EMBED_ENDPOINT_ENV, EMBED_MODEL_PATH_ENV, OPENAI_API_KEY_ENV,
};
pub use error::{EmbedError, Result};
pub use module::{EmbedConfig, EmbedModule};
pub use search::{
    Bm25Strategy, HybridStrategy, SearchStrategy, SearchStrategyType, SemanticStrategy,
};
pub use storage::{EmbeddingRecord, EmbeddingStorage};

/// Expected embedding dimension (DDD §5.9: `FLOAT[384]`).
pub const EMBEDDING_DIM: usize = 384;

/// Test-only lock to serialize environment-variable manipulation across
/// `client::tests` and `config::tests` (prevents `std::env::set_var` races).
#[cfg(test)]
pub(crate) static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_dim_is_384() {
        assert_eq!(EMBEDDING_DIM, 384, "DDD §5.9 requires FLOAT[384]");
    }
}
