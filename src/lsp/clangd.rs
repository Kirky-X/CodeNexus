// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! clangd LSP client for C/C++.
//!
//! The client implementation is shared — see [`super::server_spec::ServerClient`].
//! This module only supplies the server identity ([`ClangdSpec`])
//! and the historical public type name ([`ClangdClient`]); its tests exercise
//! the shared implementation through this adapter's concrete alias.

use super::server_spec::{LspServerSpec, ServerClient};

const DEFAULT_SERVER_PATH: &str = "clangd";

/// clangd server specification (binary: `clangd`).
pub struct ClangdSpec;

impl LspServerSpec for ClangdSpec {
    const DEFAULT_SERVER_PATH: &'static str = DEFAULT_SERVER_PATH;
}

/// C/C++ LSP client backed by `clangd`.
pub type ClangdClient = ServerClient<ClangdSpec>;

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
            ClangdClient::new().server_path,
            PathBuf::from(DEFAULT_SERVER_PATH)
        );
    }

    #[test]
    #[ignore = "requires clangd on PATH; run with --ignored"]
    fn integration_start_shutdown() {
        if std::process::Command::new("clangd")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            return;
        }
        let ws = tempfile::TempDir::new().unwrap();
        std::fs::write(ws.path().join("test.c"), "int main() { return 0; }").unwrap();
        let c = ClangdClient::new();
        c.start(ws.path()).unwrap();
        c.shutdown().unwrap();
    }
}
