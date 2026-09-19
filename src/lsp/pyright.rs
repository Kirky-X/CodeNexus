// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! pyright LSP client for Python.
//!
//! The client implementation is shared — see [`super::server_spec::ServerClient`].
//! This module only supplies the server identity ([`PyrightSpec`])
//! and the historical public type name ([`PyrightClient`]); its tests exercise
//! the shared implementation through this adapter's concrete alias.

use super::server_spec::{LspServerSpec, ServerClient};

const DEFAULT_SERVER_PATH: &str = "pyright-langserver";

/// pyright server specification (binary: `pyright-langserver`).
pub struct PyrightSpec;

impl LspServerSpec for PyrightSpec {
    const DEFAULT_SERVER_PATH: &'static str = DEFAULT_SERVER_PATH;
}

/// Python LSP client backed by `pyright-langserver`.
pub type PyrightClient = ServerClient<PyrightSpec>;

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
            PyrightClient::new().server_path,
            PathBuf::from(DEFAULT_SERVER_PATH)
        );
    }

    #[test]
    #[ignore = "requires pyright-langserver on PATH; run with --ignored"]
    fn integration_start_shutdown() {
        if std::process::Command::new("pyright-langserver")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            return;
        }
        let ws = tempfile::TempDir::new().unwrap();
        std::fs::write(ws.path().join("test.py"), "x = 1").unwrap();
        let c = PyrightClient::new();
        c.start(ws.path()).unwrap();
        c.shutdown().unwrap();
    }
}
