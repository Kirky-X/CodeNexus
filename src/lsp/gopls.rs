// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! gopls LSP client for Go.
//!
//! The client implementation is shared — see [`super::server_spec::ServerClient`].
//! This module only supplies the server identity ([`GoplsSpec`])
//! and the historical public type name ([`GoplsClient`]); its tests exercise
//! the shared implementation through this adapter's concrete alias.

use super::server_spec::{LspServerSpec, ServerClient};

const DEFAULT_SERVER_PATH: &str = "gopls";

/// gopls server specification (binary: `gopls`).
pub struct GoplsSpec;

impl LspServerSpec for GoplsSpec {
    const DEFAULT_SERVER_PATH: &'static str = DEFAULT_SERVER_PATH;
}

/// Go LSP client backed by `gopls`.
pub type GoplsClient = ServerClient<GoplsSpec>;

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
            GoplsClient::new().server_path,
            PathBuf::from(DEFAULT_SERVER_PATH)
        );
    }

    #[test]
    #[ignore = "requires gopls on PATH; run with --ignored"]
    fn integration_start_shutdown() {
        if std::process::Command::new("gopls")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            return;
        }
        let ws = tempfile::TempDir::new().unwrap();
        std::fs::write(ws.path().join("go.mod"), "module test\n\ngo 1.21\n").unwrap();
        std::fs::write(
            ws.path().join("test.go"),
            "package main\n\nfunc main() {}\n",
        )
        .unwrap();
        let c = GoplsClient::new();
        c.start(ws.path()).unwrap();
        c.shutdown().unwrap();
    }
}
