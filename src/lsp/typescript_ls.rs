// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! typescript_ls LSP client for TypeScript/JavaScript.
//!
//! The client implementation is shared — see [`super::server_spec::ServerClient`].
//! This module only supplies the server identity ([`TypeScriptLsSpec`])
//! and the historical public type name ([`TypeScriptLanguageClient`]); its tests exercise
//! the shared implementation through this adapter's concrete alias.

use super::server_spec::{LspServerSpec, ServerClient};

const DEFAULT_SERVER_PATH: &str = "typescript-language-server";

/// typescript_ls server specification (binary: `typescript-language-server`).
pub struct TypeScriptLsSpec;

impl LspServerSpec for TypeScriptLsSpec {
    const DEFAULT_SERVER_PATH: &'static str = DEFAULT_SERVER_PATH;
}

/// TypeScript/JavaScript LSP client backed by `typescript-language-server`.
pub type TypeScriptLanguageClient = ServerClient<TypeScriptLsSpec>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::LspProvider;
    use std::path::PathBuf;

    // Default-path wiring for THIS adapter's spec + const (the mechanism
    // tests live in `server_spec.rs` and run against the representative
    // `GoplsSpec`).

    #[test]
    fn new_uses_default_server_path() {
        assert_eq!(
            TypeScriptLanguageClient::new().server_path,
            PathBuf::from(DEFAULT_SERVER_PATH)
        );
    }

    #[test]
    #[ignore = "requires typescript-language-server on PATH; run with --ignored"]
    fn integration_start_shutdown() {
        if std::process::Command::new("typescript-language-server")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            return;
        }
        let ws = tempfile::TempDir::new().unwrap();
        std::fs::write(ws.path().join("test.ts"), "const x: number = 1;\n").unwrap();
        let c = TypeScriptLanguageClient::new();
        c.start(ws.path()).unwrap();
        c.shutdown().unwrap();
    }
}
