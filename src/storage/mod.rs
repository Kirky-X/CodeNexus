// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! LadybugDB persistence layer.
//!
//! Encapsulates the [`lbug`] connection, schema DDL generation (DDD §12.1),
//! CSV bulk loading (ADR-014), Cypher query execution, and the Repository
//! pattern abstraction for data access.

pub mod capability;
pub mod connection;
pub mod error;
pub mod loader;
pub mod module;
pub mod quality;
pub mod repository;
pub mod schema;

pub use connection::{SchemaInitReport, StorageConnection};
pub use error::{is_table_missing_error, Result, StorageError};
pub use loader::CsvLoader;
pub use module::{StorageConfig, StorageModule};
pub use quality::{QualityChecker, QualityReport, QualityViolation};
pub use repository::{FunctionRecord, ProjectRecord, Repository};
pub use schema::{
    all_init_ddl, embedding_table_ddl, index_ddl, node_table_columns, node_table_ddl,
    relation_table_columns, relation_table_ddl,
};
