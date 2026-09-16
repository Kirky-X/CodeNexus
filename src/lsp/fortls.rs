// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! fortls LSP client for Fortran.
//!
//! The client implementation is shared — see [`super::server_spec::ServerClient`].
//! This module only supplies the server identity ([`FortlsSpec`])
//! and the historical public type name ([`FortlsClient`]); its tests exercise
//! the shared implementation through this adapter's concrete alias.

use super::server_spec::{LspServerSpec, ServerClient};

const DEFAULT_SERVER_PATH: &str = "fortls";

/// fortls server specification (binary: `fortls`).
pub struct FortlsSpec;

impl LspServerSpec for FortlsSpec {
    const DEFAULT_SERVER_PATH: &'static str = DEFAULT_SERVER_PATH;
}

/// Fortran LSP client backed by `fortls`.
pub type FortlsClient = ServerClient<FortlsSpec>;

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
            FortlsClient::new().server_path,
            PathBuf::from(DEFAULT_SERVER_PATH)
        );
    }

    #[test]
    #[ignore = "requires fortls on PATH; run with --ignored"]
    fn integration_start_shutdown() {
        if std::process::Command::new("fortls")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            return;
        }
        let ws = tempfile::TempDir::new().unwrap();
        std::fs::write(
            ws.path().join("test.f90"),
            "program test\nend program test\n",
        )
        .unwrap();
        let c = FortlsClient::new();
        c.start(ws.path()).unwrap();
        c.shutdown().unwrap();
    }
}
