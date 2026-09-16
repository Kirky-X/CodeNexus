// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! jdtls LSP client for Java.
//!
//! The client implementation is shared — see [`super::server_spec::ServerClient`].
//! This module only supplies the server identity ([`JdtlsSpec`])
//! and the historical public type name ([`JdtlsClient`]); its tests exercise
//! the shared implementation through this adapter's concrete alias.

use super::server_spec::{LspServerSpec, ServerClient};

const DEFAULT_SERVER_PATH: &str = "jdtls";

/// jdtls server specification (binary: `jdtls`).
pub struct JdtlsSpec;

impl LspServerSpec for JdtlsSpec {
    const DEFAULT_SERVER_PATH: &'static str = DEFAULT_SERVER_PATH;
}

/// Java LSP client backed by `jdtls`.
pub type JdtlsClient = ServerClient<JdtlsSpec>;

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
            JdtlsClient::new().server_path,
            PathBuf::from(DEFAULT_SERVER_PATH)
        );
    }

    #[test]
    #[ignore = "requires jdtls on PATH; run with --ignored"]
    fn integration_start_shutdown() {
        if std::process::Command::new("jdtls")
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
            ws.path().join("Test.java"),
            "public class Test { public static void main(String[] args) {} }\n",
        )
        .unwrap();
        let c = JdtlsClient::new();
        c.start(ws.path()).unwrap();
        c.shutdown().unwrap();
    }
}
