// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

use std::time::Duration;

use serde::Serialize;

use crate::kit::{AsyncKit, AsyncReady};
use crate::service::error::CodeNexusError;
#[cfg(feature = "cli")]
use crate::service::error::{to_api_error, wrap_error};
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
///
/// Only `action` and `url` are required. `name`/`project`/`token` carry the
/// empty sentinel when omitted (see `main::SENTINEL_DEFAULTS`), so the
/// action-specific requirements are validated here:
/// `push` needs `project` (the artifact's source and registry grouping),
/// `pull` needs `name`; `list` accepts an empty `project` (all projects).
pub fn run_hub(
    kit: &AsyncKit<AsyncReady>,
    action: &str,
    url: &str,
    name: &str,
    project: &str,
    token: &str,
) -> Result<HubOutput, CodeNexusError> {
    match action.trim() {
        "push" => {
            if project.trim().is_empty() {
                return Err(CodeNexusError::InvalidInput(
                    "`hub --action push` requires --project".to_string(),
                ));
            }
        }
        "pull" => {
            if name.trim().is_empty() {
                return Err(CodeNexusError::InvalidInput(
                    "`hub --action pull` requires --name".to_string(),
                ));
            }
        }
        "list" => {}
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
    description = "Client for a CodeNexus artifact registry (protocol v1, see skill/references/commands.md). push uploads the project's .cnxp artifact; pull downloads and imports the latest artifact for a name; list queries registry artifacts. Token: --token or CODENEXUS_HUB_TOKEN. Params: action — push|pull|list (required); url — registry base (required); name — artifact name (pull); project — project name (push/list).",
    cli = true
)]
async fn hub(
    action: String,
    url: String,
    name: String,
    project: String,
    token: String,
) -> Result<(), ApiError> {
    // `HubClient` is a *blocking* reqwest client; its internal runtime panics
    // when dropped inside the handler's async runtime context (tokio:
    // "Cannot drop a runtime in a context where blocking is not allowed").
    // Run the whole blocking operation on the blocking pool, which is not in
    // an async context.
    let output = tokio::task::spawn_blocking(move || {
        let kit = kit().ok_or_else(CodeNexusError::kit_not_initialized)?;
        run_hub(&kit, &action, &url, &name, &project, &token)
    })
    .await
    .map_err(|e| {
        to_api_error(
            CodeNexusError::Internal(format!("hub worker thread join failed: {e}")),
            "hub_error",
        )
    })?
    .map_err(|e| to_api_error(e, "hub_error"))?;
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    /// State shared with the mock registry thread.
    #[derive(Default)]
    struct RegistryState {
        /// Last body uploaded via `POST /artifacts`.
        stored: Mutex<Option<Vec<u8>>>,
        /// Value of the last `X-Codenexus-Project` request header.
        project_header: Mutex<Option<String>>,
        /// Last `Authorization` request header.
        auth_header: Mutex<Option<String>>,
    }

    /// Minimal in-process registry implementing the three endpoints the client
    /// speaks (protocol v1): `POST /artifacts`,
    /// `GET /artifacts/{name}/latest`, `GET /artifacts?project=<p>`.
    ///
    /// Returns the base URL, the shared state, the stop flag and the server
    /// thread handle (stop + join at the end of the test).
    fn spawn_mock_registry() -> (
        String,
        Arc<RegistryState>,
        Arc<AtomicBool>,
        std::thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock registry");
        let addr = listener.local_addr().expect("local addr");
        let state = Arc::new(RegistryState::default());
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_state = Arc::clone(&state);
        let thread_shutdown = Arc::clone(&shutdown);
        let handle = std::thread::spawn(move || {
            listener
                .set_nonblocking(true)
                .expect("mock registry nonblocking");
            while !thread_shutdown.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        handle_connection(&mut stream, &thread_state);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        (format!("http://{addr}"), state, shutdown, handle)
    }

    /// Reads one HTTP/1.1 request and writes one response.
    fn handle_connection(stream: &mut std::net::TcpStream, state: &RegistryState) {
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .expect("read timeout");
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        // Read until the end of the header block.
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut chunk) {
                Ok(0) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => return,
            }
        }
        let header_end = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("header terminator")
            + 4;
        let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
        let request_line = headers.lines().next().unwrap_or_default().to_string();
        let content_length: usize = headers
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.eq_ignore_ascii_case("content-length")
                    .then(|| v.trim().parse().ok())?
            })
            .unwrap_or(0);
        let mut body = buf[header_end..].to_vec();
        while body.len() < content_length {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => body.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }
        *state.project_header.lock().unwrap() = headers.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("x-codenexus-project")
                .then(|| v.trim().to_string())
        });
        *state.auth_header.lock().unwrap() = headers.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("authorization")
                .then(|| v.trim().to_string())
        });

        let (status, content_type, payload): (&str, &str, Vec<u8>) = if request_line
            .starts_with("POST /artifacts ")
        {
            *state.stored.lock().unwrap() = Some(body);
            (
                "200 OK",
                "application/json",
                br#"{"id":"art_1","url":"/artifacts/art_1"}"#.to_vec(),
            )
        } else if request_line.starts_with("GET /artifacts?") {
            (
                "200 OK",
                "application/json",
                br#"[{"id":"art_1","name":"demo"}]"#.to_vec(),
            )
        } else if request_line.starts_with("GET /artifacts/") && request_line.contains("/latest") {
            match state.stored.lock().unwrap().clone() {
                Some(bytes) => ("200 OK", "application/octet-stream", bytes),
                None => ("404 Not Found", "application/json", b"{}".to_vec()),
            }
        } else {
            ("404 Not Found", "application/json", b"{}".to_vec())
        };

        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.write_all(&payload);
        let _ = stream.flush();
    }

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

    /// `--name` / `--project` / `--token` carry the empty sentinel when the
    /// user omits them; the per-action requirements surface as InvalidInput
    /// before any network call (documented contract: only action/url are
    /// required).
    #[test]
    fn run_hub_validates_action_specific_required_params() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = crate::kit::KitBootstrapConfig::new(dir.path().join("hub_db"));
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(crate::kit::build_kit(&config))
            .unwrap();

        let err = run_hub(&kit, "push", "https://hub.example.com", "", "", "")
            .expect_err("push without --project");
        assert!(err.to_string().contains("requires --project"), "got: {err}");

        let err = run_hub(&kit, "pull", "https://hub.example.com", "", "", "")
            .expect_err("pull without --name");
        assert!(err.to_string().contains("requires --name"), "got: {err}");
    }

    /// End-to-end `push` → `pull` → `list` against a live (in-process) HTTP
    /// registry: push exports the project's `.cnxp` and uploads it, pull
    /// downloads the same bytes and imports them (BLAKE3 + size validation),
    /// list queries the registry. Verifies the documented receipt shape
    /// `{action, url, artifact, project, outcome, details}` and the v1
    /// request headers.
    #[test]
    fn run_hub_push_pull_list_round_trip() {
        use crate::kit::{build_kit, KitBootstrapConfig, StorageModule};

        let (base_url, registry, shutdown, server) = spawn_mock_registry();

        let src_dir = tempfile::TempDir::new().unwrap();
        let src_config = KitBootstrapConfig::new(src_dir.path().join("hub_src_db"));
        let src_kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&src_config))
            .unwrap();
        let storage = src_kit.require::<StorageModule>().expect("storage");
        storage
            .execute("CREATE (:Project {id: 'proj_demo', name: 'demo', rootPath: '/demo', language: 'rust', fileCount: 1, indexedAt: 1000, lastCommit: 'abc'});")
            .expect("create project");

        // --- push ---
        let pushed = run_hub(&src_kit, "push", &base_url, "demo", "demo", "secret")
            .expect("push should succeed");
        assert_eq!(pushed.action, "push");
        assert_eq!(pushed.url, base_url);
        assert_eq!(pushed.project, "demo");
        assert_eq!(pushed.outcome, "ok");
        assert_eq!(
            pushed.details.get("id").and_then(|v| v.as_str()),
            Some("art_1"),
            "push receipt must carry the registry's id: {}",
            pushed.details
        );
        assert_eq!(
            registry.project_header.lock().unwrap().as_deref(),
            Some("demo"),
            "push must send X-Codenexus-Project"
        );
        assert_eq!(
            registry.auth_header.lock().unwrap().as_deref(),
            Some("Bearer secret"),
            "push must send the bearer token"
        );
        let stored = registry
            .stored
            .lock()
            .unwrap()
            .clone()
            .expect("push must upload the .cnxp body");
        assert!(!stored.is_empty(), "uploaded artifact must not be empty");

        // --- pull into a fresh DB (round-trips the uploaded bytes) ---
        let dst_dir = tempfile::TempDir::new().unwrap();
        let dst_config = KitBootstrapConfig::new(dst_dir.path().join("hub_dst_db"));
        let dst_kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&dst_config))
            .unwrap();
        let pulled =
            run_hub(&dst_kit, "pull", &base_url, "demo", "", "").expect("pull should succeed");
        assert_eq!(pulled.action, "pull");
        assert_eq!(pulled.artifact, "demo");
        assert_eq!(pulled.outcome, "ok");
        assert!(
            pulled.details.get("db_size").is_some(),
            "pull receipt must carry the import receipt: {}",
            pulled.details
        );

        // --- list ---
        let listed =
            run_hub(&src_kit, "list", &base_url, "", "demo", "").expect("list should succeed");
        assert_eq!(listed.action, "list");
        assert_eq!(listed.project, "demo");
        assert_eq!(listed.outcome, "ok");
        let artifacts = listed
            .details
            .as_array()
            .expect("list details must be the registry's JSON array");
        assert_eq!(artifacts.len(), 1);
        assert_eq!(
            artifacts[0].get("name").and_then(|v| v.as_str()),
            Some("demo")
        );

        shutdown.store(true, Ordering::SeqCst);
        server.join().expect("mock registry thread");
    }
}
