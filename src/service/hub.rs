// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

use std::time::Duration;

use serde::Serialize;

use crate::kit::{AsyncKit, AsyncReady};
use crate::service::error::CodeNexusError;
#[cfg(feature = "cli")]
use crate::service::error::{kit_not_initialized, to_api_error, wrap_error};
#[cfg(feature = "cli")]
use crate::service::export::run_export;
#[cfg(feature = "cli")]
use crate::service::import::run_import;
#[cfg(feature = "cli")]
use crate::service::runtime::kit;

#[cfg(feature = "cli")]
use sdforge::forge;
#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;

/// Percent-encodes a URL path/query value. `keep` lists extra safe bytes
/// (e.g. `/` inside path segments).
fn percent_encode(raw: &str, keep: &[u8]) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b if keep.contains(b) => out.push(*b as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Registry client (blocking reqwest, 30s timeout).
pub struct HubClient {
    base_url: String,
    token: Option<String>,
    http: reqwest::blocking::Client,
}

impl HubClient {
    /// Creates a client for a registry base URL (no trailing slash).
    pub fn new(base_url: &str, token: Option<String>) -> Result<Self, CodeNexusError> {
        let base_url = base_url.trim_end_matches('/').to_string();
        if base_url.is_empty()
            || !base_url.starts_with("http://") && !base_url.starts_with("https://")
        {
            return Err(CodeNexusError::InvalidInput(format!(
                "invalid hub URL: {base_url} (expected http(s)://host)"
            )));
        }
        Ok(Self {
            base_url,
            token,
            http: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|e| CodeNexusError::Internal(format!("http client: {e}")))?,
        })
    }

    /// Resolves the bearer token: explicit value, else `CODENEXUS_HUB_TOKEN`.
    pub fn resolve_token(explicit: &str) -> Option<String> {
        if !explicit.trim().is_empty() {
            return Some(explicit.trim().to_string());
        }
        std::env::var("CODENEXUS_HUB_TOKEN")
            .ok()
            .filter(|t| !t.is_empty())
    }

    /// URL for an operation (exposed for tests).
    fn artifacts_url(&self) -> String {
        format!("{}/artifacts", self.base_url)
    }

    fn list_url(&self, project: &str) -> String {
        if project.trim().is_empty() {
            self.artifacts_url()
        } else {
            format!(
                "{}/artifacts?project={}",
                self.base_url,
                percent_encode(project.trim(), b"")
            )
        }
    }

    fn latest_url(&self, name: &str) -> String {
        format!(
            "{}/artifacts/{}/latest",
            self.base_url,
            percent_encode(name, b"/")
        )
    }

    fn apply_auth(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    /// Pushes the exported artifact for `project` to the registry.
    pub fn push(
        &self,
        kit: &AsyncKit<AsyncReady>,
        project: &str,
    ) -> Result<HubOutcome, CodeNexusError> {
        let temp = tempfile::TempDir::new().map_err(CodeNexusError::Io)?;
        let artifact_path = temp.path().join("artifact.cnxp");
        run_export(kit, artifact_path.to_str().unwrap_or_default(), project)?;
        let bytes = std::fs::read(&artifact_path).map_err(CodeNexusError::Io)?;

        let mut request = self
            .http
            .post(self.artifacts_url())
            .header("Content-Type", "application/octet-stream")
            .header("X-Codenexus-Project", project)
            .body(bytes);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .map_err(|e| CodeNexusError::Internal(format!("hub push failed: {e}")))?;
        let status = response.status();
        let text = response.text().unwrap_or_default();
        Self::map_status(status, &text)?;
        let payload: serde_json::Value =
            serde_json::from_str(&text).unwrap_or(serde_json::json!({ "raw": text }));
        Ok(HubOutcome { payload })
    }

    /// Pulls the latest artifact for `name` into the local DB.
    pub fn pull(
        &self,
        kit: &AsyncKit<AsyncReady>,
        name: &str,
    ) -> Result<HubOutcome, CodeNexusError> {
        let response = self
            .apply_auth(self.http.get(self.latest_url(name)))
            .send()
            .map_err(|e| CodeNexusError::Internal(format!("hub pull failed: {e}")))?;
        Self::map_status(response.status(), "")?;
        let bytes = response
            .bytes()
            .map_err(|e| CodeNexusError::Internal(format!("hub pull body: {e}")))?;
        let temp = tempfile::TempDir::new().map_err(CodeNexusError::Io)?;
        let artifact_path = temp.path().join("pulled.cnxp");
        std::fs::write(&artifact_path, &bytes).map_err(CodeNexusError::Io)?;
        // run_import validates the .cnxp container (BLAKE3 + size caps).
        let import_result = run_import(
            kit,
            artifact_path.to_str().unwrap_or_default(),
            false,
            "",
            "",
        )?;
        let payload = serde_json::to_value(&import_result)
            .map_err(|e| CodeNexusError::Internal(format!("import receipt: {e}")))?;
        Ok(HubOutcome { payload })
    }

    /// Lists registry artifacts for a project (empty = all).
    pub fn list(&self, project: &str) -> Result<HubOutcome, CodeNexusError> {
        let response = self
            .apply_auth(self.http.get(self.list_url(project)))
            .send()
            .map_err(|e| CodeNexusError::Internal(format!("hub list failed: {e}")))?;
        let status = response.status();
        let text = response.text().unwrap_or_default();
        Self::map_status(status, &text)?;
        let payload: serde_json::Value =
            serde_json::from_str(&text).unwrap_or(serde_json::json!({ "raw": text }));
        Ok(HubOutcome { payload })
    }

    /// Maps non-2xx responses onto InvalidInput with status + body summary.
    fn map_status(status: reqwest::StatusCode, body: &str) -> Result<(), CodeNexusError> {
        if status.is_success() {
            return Ok(());
        }
        let summary: String = body.chars().take(200).collect();
        Err(CodeNexusError::InvalidInput(format!(
            "hub request failed: HTTP {status} {summary}"
        )))
    }
}

/// Outcome payload of a hub operation.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HubOutcome {
    pub payload: serde_json::Value,
}

/// JSON-serializable `hub` output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HubOutput {
    pub action: String,
    pub url: String,
    pub artifact: String,
    pub project: String,
    pub outcome: String,
    pub details: serde_json::Value,
}

/// Core logic — dispatches a hub action (testable core).
pub fn run_hub(
    kit: &AsyncKit<AsyncReady>,
    action: &str,
    url: &str,
    name: &str,
    project: &str,
    token: &str,
) -> Result<HubOutput, CodeNexusError> {
    match action.trim() {
        "push" | "pull" | "list" => {}
        other => {
            return Err(CodeNexusError::InvalidInput(format!(
                "invalid --action: {other} (expected push|pull|list)"
            )))
        }
    }
    let token_value = HubClient::resolve_token(token);
    let client = HubClient::new(url, token_value)?;
    let outcome = match action.trim() {
        "push" => client.push(kit, project)?,
        "pull" => client.pull(kit, name)?,
        _ => client.list(project)?,
    };
    Ok(HubOutput {
        action: action.trim().to_string(),
        url: url.to_string(),
        artifact: name.to_string(),
        project: project.to_string(),
        outcome: "ok".to_string(),
        details: outcome.payload,
    })
}

/// CLI wrapper — prints the hub receipt to stdout as JSON.
#[cfg(feature = "cli")]
#[forge(
    name = "hub",
    version = "0.4.0",
    description = "Client for a CodeNexus artifact registry (protocol v1, see docs/HUB_PROTOCOL.md). push uploads the project's .cnxp artifact; pull downloads and imports the latest artifact for a name; list queries registry artifacts. Token: --token or CODENEXUS_HUB_TOKEN. Params: action — push|pull|list (required); url — registry base (required); name — artifact name (pull); project — project name (push/list).",
    cli = true
)]
async fn hub(
    action: String,
    url: String,
    name: String,
    project: String,
    token: String,
) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let output = run_hub(&kit, &action, &url, &name, &project, &token)
        .map_err(|e| to_api_error(e, "hub_error"))?;
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_rejects_bad_urls() {
        assert!(HubClient::new("", None).is_err());
        assert!(HubClient::new("ftp://x", None).is_err());
        let client = HubClient::new("https://hub.example.com/", None).unwrap();
        assert_eq!(
            client.base_url, "https://hub.example.com",
            "trailing slash trimmed"
        );
    }

    #[test]
    fn urls_are_built_per_operation() {
        let client = HubClient::new("https://hub.example.com", None).unwrap();
        assert_eq!(client.artifacts_url(), "https://hub.example.com/artifacts");
        assert_eq!(
            client.latest_url("my artifact"),
            "https://hub.example.com/artifacts/my%20artifact/latest"
        );
        assert_eq!(
            client.list_url("demo"),
            "https://hub.example.com/artifacts?project=demo"
        );
        assert_eq!(client.list_url(""), "https://hub.example.com/artifacts");
    }

    #[test]
    fn token_prefers_explicit_over_env() {
        // Explicit value wins regardless of environment (may be unset here).
        let token = HubClient::resolve_token("explicit-token");
        assert_eq!(token.as_deref(), Some("explicit-token"));
        // Whitespace-only explicit value falls through to env (absent here).
        std::env::remove_var("CODENEXUS_HUB_TOKEN");
        assert_eq!(HubClient::resolve_token("  "), None);
    }

    #[test]
    fn run_hub_rejects_unknown_action_before_network() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = crate::kit::KitBootstrapConfig::new(dir.path().join("hub_db"));
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(crate::kit::build_kit(&config))
            .unwrap();
        let err = run_hub(&kit, "teleport", "https://hub.example.com", "", "", "")
            .expect_err("unknown action");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
        assert!(err.to_string().contains("invalid --action"));
    }
}
