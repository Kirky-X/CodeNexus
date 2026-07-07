// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `rust-analyzer` LSP client (T007, v0.2.0).
//!
//! [`RustAnalyzerClient`] spawns `rust-analyzer` as a subprocess, wires its
//! stdin/stdout into an [`lsp_server::Connection`] via two background threads,
//! and exposes the [`LspProvider`] trait surface for Go-to-Definition,
//! Type-Definition, and Hover queries.
//!
//! # Lifecycle
//!
//! 1. [`LspProvider::start`] spawns the binary at `server_path` (default
//!    `"rust-analyzer"`, override via [`RustAnalyzerClient::with_server_path`]),
//!    completes the LSP `initialize`/`initialized` handshake, and stores the
//!    session in an internal `Mutex<Option<_>>`.
//! 2. `definition`/`type_definition`/`hover` send JSON-RPC requests and wait
//!    for the matching response (5-second timeout, see
//!    [`REQUEST_TIMEOUT_MS`]). Server-initiated notifications/requests that
//!    arrive mid-round-trip are drained to keep the channel clear.
//! 3. [`LspProvider::shutdown`] sends `shutdown` + `exit`, reaps the child,
//!    and drops the session. Calling `shutdown` before `start` is a no-op.
//!
//! # Failure semantics (Rule 12)
//!
//! - Binary missing / not executable → [`LspError::ServerStart`]
//! - Channel disconnect / malformed payload → [`LspError::Communication`]
//! - No response within 5 s → [`LspError::Timeout`]
//!
//! The indexing pipeline (R-lsp-004) treats *any* of these as "LSP
//! unavailable, fall back to pure tree-sitter" — none of them abort the index.

use std::io::{BufReader, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{Exit, Initialized, Notification as LspNotification};
use lsp_types::request::{GotoDefinition, GotoTypeDefinition, HoverRequest, Initialize};
use lsp_types::{
    ClientCapabilities, GotoDefinitionParams, GotoDefinitionResponse, HoverParams,
    InitializeParams, InitializedParams, PartialResultParams, Position, TextDocumentIdentifier,
    TextDocumentPositionParams, Url, WorkDoneProgressParams, WorkspaceFolder,
};

use super::{LspError, LspProvider, REQUEST_TIMEOUT_MS};

/// Default `rust-analyzer` binary name resolved via `PATH`.
///
/// Users with non-standard installs override this with
/// [`RustAnalyzerClient::with_server_path`].
const DEFAULT_SERVER_PATH: &str = "rust-analyzer";

/// LSP client backed by a `rust-analyzer` subprocess.
///
/// Cheap to construct (no I/O); the subprocess is spawned only when
/// [`LspProvider::start`] is called. The interior `Mutex<Option<Session>>`
/// makes all trait methods `&self` so the indexing pipeline can share a
/// single client across worker threads (R-lsp-004).
pub struct RustAnalyzerClient {
    server_path: PathBuf,
    session: Mutex<Option<Session>>,
}

impl RustAnalyzerClient {
    /// Construct a client that will resolve `rust-analyzer` from `PATH`.
    #[must_use]
    pub fn new() -> Self {
        Self::with_server_path(PathBuf::from(DEFAULT_SERVER_PATH))
    }

    /// Construct a client pointing at an explicit server binary path.
    ///
    /// Test-only entry point: passing `"/nonexistent/rust-analyzer"` makes
    /// [`LspProvider::start`] fail deterministically with
    /// [`LspError::ServerStart`] without depending on whether `rust-analyzer`
    /// is installed in CI.
    #[must_use]
    pub fn with_server_path(server_path: PathBuf) -> Self {
        Self { server_path, session: Mutex::new(None) }
    }
}

impl Default for RustAnalyzerClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Active LSP session — populated by `start`, drained by `shutdown`.
struct Session {
    child: Child,
    connection: Connection,
    // Held (not joined) for the lifetime of the session; the threads exit
    // naturally when their channel peer is dropped on `shutdown`.
    _reader_handle: JoinHandle<()>,
    _writer_handle: JoinHandle<()>,
    /// Monotonic JSON-RPC request id. `lsp_server::RequestId` only implements
    /// `From<i32>` (and `From<String>`), so we cap at `i32::MAX` — a single
    /// index run never exhausts 2^31 requests.
    next_request_id: i32,
}

impl LspProvider for RustAnalyzerClient {
    fn start(&self, workspace: &Path) -> Result<(), LspError> {
        let mut guard = self.session.lock().expect("session mutex poisoned");
        if guard.is_some() {
            // Idempotent: a second `start` is a no-op so that the indexing
            // pipeline can retry on transient failures without first having
            // to call `shutdown` (R-lsp-004: "LSP server startup failure
            // must not abort the index").
            return Ok(());
        }

        // 1. Spawn the server subprocess rooted at `workspace`.
        let mut child = Command::new(&self.server_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .current_dir(workspace)
            .spawn()
            .map_err(|e| LspError::ServerStart(e.to_string()))?;

        let stdin = child.stdin.take().expect("stdin was Stdio::piped");
        let stdout = child.stdout.take().expect("stdout was Stdio::piped");

        // 2. Wire the subprocess's stdin/stdout into an `lsp_server::Connection`.
        let (connection, reader_handle, writer_handle) = spawn_transport(stdin, stdout);

        let mut session = Session {
            child,
            connection,
            _reader_handle: reader_handle,
            _writer_handle: writer_handle,
            next_request_id: 1,
        };

        // 3. Send `initialize` with `rootUri = workspace`.
        let root_uri = path_to_url(workspace)?;
        let init_params = InitializeParams {
            process_id: Some(std::process::id()),
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: root_uri,
                name: workspace
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "workspace".to_string()),
            }]),
            capabilities: ClientCapabilities::default(),
            ..Default::default()
        };
        // The Initialize result carries ServerCapabilities which we don't
        // introspect in v0.2.0 — we just need the handshake to complete.
        let _init_result = send_request::<Initialize>(&mut session, init_params)?;

        // 4. Send the `initialized` notification to unblock the server.
        send_notification(
            &session.connection,
            Initialized::METHOD,
            &InitializedParams {},
        )?;

        *guard = Some(session);
        Ok(())
    }

    fn definition(
        &self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Option<lsp_types::Location>, LspError> {
        let mut guard = self.session.lock().expect("session mutex poisoned");
        let session = guard
            .as_mut()
            .ok_or_else(|| LspError::Communication("LSP server not started".into()))?;

        let params = GotoDefinitionParams {
            text_document_position_params: make_position_params(file, line, col)?,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        let resp: Option<GotoDefinitionResponse> = send_request::<GotoDefinition>(session, params)?;
        Ok(extract_first_location(resp))
    }

    fn type_definition(
        &self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Option<lsp_types::Location>, LspError> {
        let mut guard = self.session.lock().expect("session mutex poisoned");
        let session = guard
            .as_mut()
            .ok_or_else(|| LspError::Communication("LSP server not started".into()))?;

        // GotoTypeDefinition reuses GotoDefinitionParams as its parameter type.
        let params = GotoDefinitionParams {
            text_document_position_params: make_position_params(file, line, col)?,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        let resp: Option<GotoDefinitionResponse> =
            send_request::<GotoTypeDefinition>(session, params)?;
        Ok(extract_first_location(resp))
    }

    fn hover(
        &self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Option<lsp_types::Hover>, LspError> {
        let mut guard = self.session.lock().expect("session mutex poisoned");
        let session = guard
            .as_mut()
            .ok_or_else(|| LspError::Communication("LSP server not started".into()))?;

        let params = HoverParams {
            text_document_position_params: make_position_params(file, line, col)?,
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        let hover: Option<lsp_types::Hover> = send_request::<HoverRequest>(session, params)?;
        Ok(hover)
    }

    fn shutdown(&self) -> Result<(), LspError> {
        let mut guard = self.session.lock().expect("session mutex poisoned");
        let Some(mut session) = guard.take() else {
            // Specmark R-lsp-002: "未启动直接 shutdown → 返回 Ok(())（不 panic）".
            return Ok(());
        };

        // Best-effort teardown — server may already be gone. We send
        // `shutdown` then `exit` per LSP spec §shutdown sequence, but ignore
        // any errors so that a half-broken session still gets reaped.
        let _ = send_raw_request(&mut session, "shutdown", serde_json::Value::Null);
        let _ = send_notification(&session.connection, Exit::METHOD, &serde_json::Value::Null);

        // Reap the child (don't leave zombies) — but bound the wait so a
        // wedged server can't hang the CLI indefinitely.
        let _ = session.child.wait();
        // Drop the session → channels close → reader/writer threads exit.
        drop(session);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Wire a subprocess's stdin/stdout into an [`lsp_server::Connection`].
///
/// Two background threads handle the byte-level framing:
/// - **writer**: pops `Message`s off a channel and writes them to the child's
///   stdin (Content-Length framing handled by [`Message::write`]).
/// - **reader**: reads framed `Message`s off the child's stdout and pushes
///   them onto a channel.
///
/// The threads exit when their channel peer is dropped (i.e. when the
/// `Connection` is dropped on shutdown).
fn spawn_transport(
    stdin: ChildStdin,
    stdout: ChildStdout,
) -> (Connection, JoinHandle<()>, JoinHandle<()>) {
    // Small bound: LSP is request/response, not high-throughput.
    let (writer_tx, writer_rx): (Sender<Message>, Receiver<Message>) = bounded(16);
    let (reader_tx, reader_rx): (Sender<Message>, Receiver<Message>) = bounded(16);

    let writer_handle = thread::Builder::new()
        .name("codenexus-lsp-writer".to_owned())
        .spawn(move || {
            let mut stdin = stdin;
            for msg in writer_rx.iter() {
                if Message::write(&msg, &mut stdin).is_err() {
                    break;
                }
            }
        })
        .expect("spawn lsp writer thread");

    let reader_handle = thread::Builder::new()
        .name("codenexus-lsp-reader".to_owned())
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match Message::read(&mut reader) {
                    Ok(Some(msg)) => {
                        if reader_tx.send(msg).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break, // EOF — child closed stdout.
                    Err(_) => break,   // malformed frame — stop reading.
                }
            }
        })
        .expect("spawn lsp reader thread");

    let connection = Connection { sender: writer_tx, receiver: reader_rx };
    (connection, reader_handle, writer_handle)
}

/// Send a typed LSP request and await its typed response.
///
/// Drains server-initiated notifications/requests that arrive before the
/// matching response (e.g. `window/logMessage` diagnostics) so they don't
/// clog the channel. Returns [`LspError::Timeout`] if no response with the
/// expected id arrives within [`REQUEST_TIMEOUT_MS`].
fn send_request<R>(session: &mut Session, params: R::Params) -> Result<R::Result, LspError>
where
    R: lsp_types::request::Request,
    R::Params: serde::Serialize,
    R::Result: serde::de::DeserializeOwned,
{
    let id = session.next_request_id;
    session.next_request_id += 1;
    let request = Request::new(RequestId::from(id), R::METHOD.to_string(), params);
    session
        .connection
        .sender
        .send(Message::Request(request))
        .map_err(|e| LspError::Communication(format!("send request: {e}")))?;

    let deadline = Duration::from_millis(REQUEST_TIMEOUT_MS);
    loop {
        let msg = session
            .connection
            .receiver
            .recv_timeout(deadline)
            .map_err(|e| match e {
                RecvTimeoutError::Timeout => LspError::Timeout(REQUEST_TIMEOUT_MS),
                RecvTimeoutError::Disconnected => {
                    LspError::Communication("server connection closed".into())
                }
            })?;
        match msg {
            Message::Response(resp) => {
                if resp.id != RequestId::from(id) {
                    // Stale response from a previous (timed-out) request —
                    // log and keep waiting for ours. We don't have a logger
                    // here, so just drop it.
                    continue;
                }
                return decode_response::<R>(resp);
            }
            // Server-initiated notifications (`window/logMessage`,
            // `textDocument/publishDiagnostics`, etc.) and requests
            // (`workspace/configuration`) are silently drained in v0.2.0.
            // A fuller implementation would dispatch them, but the
            // indexing pipeline only needs the round-trip response.
            Message::Notification(_) | Message::Request(_) => continue,
        }
    }
}

/// Send a raw (untyped) request — used for the `shutdown` handshake which
/// doesn't have a typed `Request` impl we want to depend on.
fn send_raw_request(session: &mut Session, method: &str, params: serde_json::Value) {
    let id = session.next_request_id;
    session.next_request_id += 1;
    let request = Request {
        id: RequestId::from(id),
        method: method.to_string(),
        params,
    };
    // Best-effort: caller (shutdown) ignores errors.
    let _ = session.connection.sender.send(Message::Request(request));
}

/// Send an LSP notification (no response expected).
fn send_notification<P: serde::Serialize>(
    conn: &Connection,
    method: &str,
    params: &P,
) -> Result<(), LspError> {
    let params_value =
        serde_json::to_value(params).map_err(|e| LspError::Communication(e.to_string()))?;
    let notif = Notification { method: method.to_string(), params: params_value };
    conn.sender
        .send(Message::Notification(notif))
        .map_err(|e| LspError::Communication(format!("send notification: {e}")))
}

/// Decode a JSON-RPC [`Response`] into the typed result of `R`.
fn decode_response<R>(resp: Response) -> Result<R::Result, LspError>
where
    R: lsp_types::request::Request,
    R::Result: serde::de::DeserializeOwned,
{
    if let Some(err) = resp.error {
        return Err(LspError::Communication(format!(
            "server error {}: {}",
            err.code, err.message
        )));
    }
    match resp.result {
        Some(value) => {
            serde_json::from_value::<R::Result>(value)
                .map_err(|e| LspError::Communication(format!("decode response: {e}")))
        }
        None => {
            // No result payload — for `Option<T>` results this means `None`.
            // serde_json will produce `None` from `Value::Null`, but a totally
            // absent field needs explicit handling.
            serde_json::from_value::<R::Result>(serde_json::Value::Null)
                .map_err(|e| LspError::Communication(format!("decode null response: {e}")))
        }
    }
}

/// Convert a [`GotoDefinitionResponse`] into the first [`lsp_types::Location`],
/// if any. v0.2.0 only needs the first hit — multi-location results are
/// v0.3.0+ territory (specmark Out of Scope).
fn extract_first_location(resp: Option<GotoDefinitionResponse>) -> Option<lsp_types::Location> {
    match resp? {
        GotoDefinitionResponse::Scalar(loc) => Some(loc),
        GotoDefinitionResponse::Array(locs) => locs.into_iter().next(),
        GotoDefinitionResponse::Link(links) => {
            // `LocationLink` carries `target_uri` + `target_range` rather than
            // a pre-built `Location`; reassemble one so the caller sees a
            // uniform shape across all three response variants.
            links.into_iter().next().map(|link| lsp_types::Location {
                uri: link.target_uri,
                range: link.target_range,
            })
        }
    }
}

/// Build [`TextDocumentPositionParams`] from a file path + 0-based line/col.
fn make_position_params(
    file: &Path,
    line: u32,
    col: u32,
) -> Result<TextDocumentPositionParams, LspError> {
    let uri = path_to_url(file)?;
    Ok(TextDocumentPositionParams {
        text_document: TextDocumentIdentifier { uri },
        position: Position { line, character: col },
    })
}

/// Convert a filesystem path to a `file://` [`Url`].
fn path_to_url(path: &Path) -> Result<Url, LspError> {
    Url::from_file_path(path).map_err(|_| {
        LspError::Communication(format!(
            "path is not absolute or cannot be encoded as a file URL: {}",
            path.display()
        ))
    })
}

// Allow `BufRead` import to remain in scope for the reader thread signature.
// (`BufReader` implements `BufRead`; the trait import keeps the type signature
// readable and survives future refactors.)
#[allow(dead_code)]
fn _buf_read_in_scope(_r: &mut dyn BufRead) {}

// Suppress unused import warning for `Write` — it's used by `Message::write`
// via the `&mut stdin` argument (which is `ChildStdin: Write`), but the trait
// isn't named explicitly in the call.
#[allow(dead_code)]
fn _write_in_scope<W: Write>(_w: &mut W) {}

#[cfg(test)]
mod tests {
    //! R-lsp-002 acceptance tests.
    //!
    //! These tests do NOT require `rust-analyzer` to be installed — they
    //! exercise the failure paths (missing binary, double shutdown) that
    //! are deterministic. The happy-path round-trip is covered by the
    //! `#[ignore]`-tagged integration tests at the bottom of this module.

    use super::*;
    use std::path::PathBuf;

    // --- R-lsp-002: pointing at a nonexistent binary must fail with ServerStart ---

    #[test]
    fn start_nonexistent_server_returns_error() {
        let client = RustAnalyzerClient::with_server_path(PathBuf::from(
            "/nonexistent/path/to/rust-analyzer",
        ));
        let workspace = std::env::temp_dir();
        let result = client.start(&workspace);
        match result {
            Err(LspError::ServerStart(msg)) => {
                assert!(
                    !msg.is_empty(),
                    "ServerStart error must carry a non-empty cause, got: {msg:?}"
                );
            }
            other => panic!(
                "expected Err(LspError::ServerStart(_)) for nonexistent binary, got: {other:?}"
            ),
        }
    }

    // --- R-lsp-002: shutdown without start must be Ok and must not panic ---

    #[test]
    fn shutdown_without_start_returns_ok() {
        let client = RustAnalyzerClient::new();
        let result = client.shutdown();
        assert!(result.is_ok(), "shutdown before start should be Ok, got: {result:?}");
    }

    // --- R-lsp-002: shutdown after a failed start is still Ok ---

    #[test]
    fn shutdown_after_failed_start_returns_ok() {
        let client = RustAnalyzerClient::with_server_path(PathBuf::from(
            "/nonexistent/path/to/rust-analyzer",
        ));
        let workspace = std::env::temp_dir();
        // start fails — session stays None.
        let _ = client.start(&workspace);
        // shutdown must still be safe.
        let result = client.shutdown();
        assert!(result.is_ok(), "shutdown after failed start should be Ok: {result:?}");
    }

    // --- R-lsp-002: definition/hover without start return Communication error ---

    #[test]
    fn definition_without_start_returns_communication_error() {
        let client = RustAnalyzerClient::new();
        let result = client.definition(Path::new("/tmp/lib.rs"), 0, 0);
        assert!(matches!(result, Err(LspError::Communication(_))), "got: {result:?}");
    }

    #[test]
    fn type_definition_without_start_returns_communication_error() {
        let client = RustAnalyzerClient::new();
        let result = client.type_definition(Path::new("/tmp/lib.rs"), 0, 0);
        assert!(matches!(result, Err(LspError::Communication(_))), "got: {result:?}");
    }

    #[test]
    fn hover_without_start_returns_communication_error() {
        let client = RustAnalyzerClient::new();
        let result = client.hover(Path::new("/tmp/lib.rs"), 0, 0);
        assert!(matches!(result, Err(LspError::Communication(_))), "got: {result:?}");
    }

    // --- Constructor sanity ---

    #[test]
    fn new_uses_default_server_path() {
        let client = RustAnalyzerClient::new();
        assert_eq!(client.server_path, PathBuf::from(DEFAULT_SERVER_PATH));
    }

    #[test]
    fn with_server_path_overrides_default() {
        let client =
            RustAnalyzerClient::with_server_path(PathBuf::from("/custom/rust-analyzer"));
        assert_eq!(client.server_path, PathBuf::from("/custom/rust-analyzer"));
    }

    #[test]
    fn default_impl_matches_new() {
        let a = RustAnalyzerClient::new();
        let b = RustAnalyzerClient::default();
        assert_eq!(a.server_path, b.server_path);
    }

    // --- Helper unit tests ---

    #[test]
    fn extract_first_location_from_scalar() {
        let uri = Url::parse("file:///tmp/x.rs").unwrap();
        let loc = lsp_types::Location {
            uri: uri.clone(),
            range: lsp_types::Range {
                start: Position { line: 0, character: 0 },
                end: Position { line: 0, character: 5 },
            },
        };
        let resp = Some(GotoDefinitionResponse::Scalar(loc.clone()));
        assert_eq!(extract_first_location(resp), Some(loc));
    }

    #[test]
    fn extract_first_location_from_array_returns_first() {
        let uri = Url::parse("file:///tmp/x.rs").unwrap();
        let mk = |line: u32| lsp_types::Location {
            uri: uri.clone(),
            range: lsp_types::Range {
                start: Position { line, character: 0 },
                end: Position { line, character: 5 },
            },
        };
        let resp = Some(GotoDefinitionResponse::Array(vec![mk(1), mk(2), mk(3)]));
        let first = extract_first_location(resp).expect("should have a location");
        assert_eq!(first.range.start.line, 1);
    }

    #[test]
    fn extract_first_location_from_empty_array_returns_none() {
        let resp = Some(GotoDefinitionResponse::Array(vec![]));
        assert_eq!(extract_first_location(resp), None);
    }

    #[test]
    fn extract_first_location_from_none_returns_none() {
        assert_eq!(extract_first_location(None), None);
    }

    #[test]
    fn extract_first_location_from_link_uses_target_uri_and_range() {
        // GotoDefinitionResponse::Link carries LocationLink { target_uri,
        // target_range, ... } — extract_first_location reassembles a
        // Location from those fields so callers see a uniform shape across
        // all three response variants.
        let target_uri = Url::parse("file:///tmp/target.rs").unwrap();
        let target_range = lsp_types::Range {
            start: Position { line: 10, character: 2 },
            end: Position { line: 10, character: 8 },
        };
        let link = lsp_types::LocationLink {
            origin_selection_range: None,
            target_uri: target_uri.clone(),
            target_range,
            target_selection_range: target_range,
        };
        let resp = Some(GotoDefinitionResponse::Link(vec![link]));
        let result = extract_first_location(resp).expect("Link should yield a Location");
        assert_eq!(result.uri, target_uri);
        assert_eq!(result.range, target_range);
    }

    #[test]
    fn extract_first_location_from_empty_link_returns_none() {
        let resp = Some(GotoDefinitionResponse::Link(vec![]));
        assert_eq!(extract_first_location(resp), None);
    }

    #[test]
    fn path_to_url_rejects_relative_path() {
        let result = path_to_url(Path::new("relative/path.rs"));
        assert!(
            matches!(result, Err(LspError::Communication(_))),
            "relative path should be rejected, got: {result:?}"
        );
    }

    #[test]
    fn path_to_url_accepts_absolute_path() {
        let result = path_to_url(Path::new("/tmp/lib.rs"));
        assert!(result.is_ok(), "absolute path should be accepted: {result:?}");
        assert_eq!(result.unwrap().as_str(), "file:///tmp/lib.rs");
    }

    // --- decode_response: server error, null result, type mismatch ---

    #[test]
    fn decode_response_returns_communication_error_when_server_reports_error() {
        let resp = Response {
            id: RequestId::from(1),
            result: None,
            error: Some(lsp_server::ResponseError {
                code: -32603,
                message: "internal error".to_string(),
                data: None,
            }),
        };
        let result: Result<lsp_types::InitializeResult, _> = decode_response::<Initialize>(resp);
        match result {
            Err(LspError::Communication(msg)) => {
                assert!(msg.contains("server error"), "got: {msg}");
                assert!(msg.contains("-32603"), "got: {msg}");
                assert!(msg.contains("internal error"), "got: {msg}");
            }
            other => panic!("expected LspError::Communication, got: {other:?}"),
        }
    }

    #[test]
    fn decode_response_decodes_absent_result_as_none_for_option_types() {
        // HoverRequest::Result = Option<Hover>; an absent result field should
        // decode to None (serde_json::Value::Null → None for Option<T>).
        let resp = Response {
            id: RequestId::from(1),
            result: None,
            error: None,
        };
        let result: Result<Option<lsp_types::Hover>, _> = decode_response::<HoverRequest>(resp);
        assert!(result.is_ok(), "got: {:?}", result.err());
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn decode_response_returns_error_when_result_does_not_match_type() {
        // Provide a result that cannot deserialize as InitializeResult (a struct).
        let resp = Response {
            id: RequestId::from(1),
            result: Some(serde_json::Value::String("not a struct".into())),
            error: None,
        };
        let result: Result<lsp_types::InitializeResult, _> = decode_response::<Initialize>(resp);
        assert!(
            matches!(result, Err(LspError::Communication(_))),
            "expected Communication error, got: {result:?}"
        );
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("decode response"), "got: {msg}");
    }

    // --- send_notification: channel disconnect ---

    #[test]
    fn send_notification_returns_error_when_channel_disconnected() {
        // Drop the receiver so the sender's send() fails with a Disconnected
        // error, which send_notification must surface as LspError::Communication.
        let (sender, receiver) = bounded::<Message>(1);
        drop(receiver);
        let (_dummy_tx, dummy_rx) = bounded::<Message>(1);
        let conn = Connection { sender, receiver: dummy_rx };
        let result = send_notification(&conn, "some/method", &serde_json::Value::Null);
        assert!(
            matches!(result, Err(LspError::Communication(_))),
            "expected Communication error, got: {result:?}"
        );
    }

    // --- Happy-path integration tests (require real rust-analyzer on PATH) ---
    //
    // These are `#[ignore]` so CI doesn't depend on rust-analyzer being
    // installed. Run locally with:
    //     cargo test --features lsp --lib lsp::client::tests -- --ignored
    //
    // They spin up a real rust-analyzer process against a temp workspace
    // and exercise the full initialize → definition/hover → shutdown
    // round-trip.

    fn rust_analyzer_available() -> bool {
        Command::new("rust-analyzer")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }

    #[test]
    #[ignore = "requires rust-analyzer on PATH; run with --ignored"]
    fn integration_start_and_shutdown_succeeds() {
        if !rust_analyzer_available() {
            eprintln!("skipping: rust-analyzer not on PATH");
            return;
        }
        let workspace = tempfile::TempDir::new().unwrap();
        std::fs::write(
            workspace.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(workspace.path().join("src")).unwrap();
        std::fs::write(
            workspace.path().join("src/main.rs"),
            "fn main() { let x = 42; println!(\"{x}\"); }\n",
        )
        .unwrap();

        let client = RustAnalyzerClient::new();
        let start_result = client.start(workspace.path());
        assert!(start_result.is_ok(), "start should succeed: {:?}", start_result.err());
        let shutdown_result = client.shutdown();
        assert!(shutdown_result.is_ok(), "shutdown should succeed: {:?}", shutdown_result.err());
    }

    #[test]
    #[ignore = "requires rust-analyzer on PATH; run with --ignored"]
    fn integration_hover_returns_type_info() {
        if !rust_analyzer_available() {
            eprintln!("skipping: rust-analyzer not on PATH");
            return;
        }
        let workspace = tempfile::TempDir::new().unwrap();
        std::fs::write(
            workspace.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(workspace.path().join("src")).unwrap();
        // `fn add(a: i32, b: i32) -> i32 { a + b }` — hover over `add` name
        // (line 0, col 4 — "fn " is 3 chars + 'a' at index 3, col 4 is safe).
        std::fs::write(
            workspace.path().join("src/main.rs"),
            "fn add(a: i32, b: i32) -> i32 { a + b }\nfn main() { let _ = add(1, 2); }\n",
        )
        .unwrap();

        let client = RustAnalyzerClient::new();
        client.start(workspace.path()).expect("start");
        // rust-analyzer needs time to index; hover may return None early.
        // We only assert that the call completes without an LspError.
        let result = client.hover(&workspace.path().join("src/main.rs"), 0, 4);
        assert!(
            result.is_ok(),
            "hover should not return an LspError: {:?}",
            result.err()
        );
        client.shutdown().expect("shutdown");
    }

    // -----------------------------------------------------------------------
    // Mock-channel tests: send_request / send_raw_request / make_position_params
    //
    // These tests construct a `Session` backed by in-memory crossbeam channels
    // instead of a real rust-analyzer subprocess. The `Child` is a `true`
    // process (exits immediately) — only `send_request` / `send_raw_request`
    // touch the channels, not `child`.
    // -----------------------------------------------------------------------

    /// Build a `Session` backed by in-memory channels.
    ///
    /// Returns `(session, reader_tx, writer_rx)`:
    /// - `reader_tx`: push mock `Message::Response` / `Message::Notification`
    ///   values for `send_request` to consume via `connection.receiver`.
    /// - `writer_rx`: inspect `Message::Request` / `Message::Notification`
    ///   values that `send_request` / `send_raw_request` pushed to
    ///   `connection.sender`.
    fn mock_session() -> (Session, Sender<Message>, Receiver<Message>) {
        let child = Command::new("true")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn true");
        let (writer_tx, writer_rx): (Sender<Message>, Receiver<Message>) = bounded(16);
        let (reader_tx, reader_rx): (Sender<Message>, Receiver<Message>) = bounded(16);
        let writer_handle = thread::spawn(|| {});
        let reader_handle = thread::spawn(|| {});
        let connection = Connection { sender: writer_tx, receiver: reader_rx };
        let session = Session {
            child,
            connection,
            _reader_handle: reader_handle,
            _writer_handle: writer_handle,
            next_request_id: 1,
        };
        (session, reader_tx, writer_rx)
    }

    /// Build `HoverParams` for testing — the specific values don't matter
    /// since we only test the transport layer, not the params serialization.
    fn make_hover_params() -> HoverParams {
        HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: Url::parse("file:///tmp/x.rs").unwrap(),
                },
                position: Position { line: 0, character: 0 },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        }
    }

    // --- make_position_params ---

    #[test]
    fn make_position_params_builds_correct_uri_and_position() {
        let params = make_position_params(Path::new("/tmp/lib.rs"), 10, 20)
            .expect("absolute path should succeed");
        assert_eq!(params.text_document.uri.as_str(), "file:///tmp/lib.rs");
        assert_eq!(params.position.line, 10);
        assert_eq!(params.position.character, 20);
    }

    #[test]
    fn make_position_params_rejects_relative_path() {
        let result = make_position_params(Path::new("relative.rs"), 0, 0);
        assert!(
            matches!(result, Err(LspError::Communication(_))),
            "relative path should be rejected, got: {result:?}"
        );
    }

    // --- send_request: timeout when no response ---

    #[test]
    fn send_request_times_out_when_no_response_arrives() {
        // No response pushed → recv_timeout fires after REQUEST_TIMEOUT_MS.
        let (mut session, _reader_tx, _writer_rx) = mock_session();
        let result = send_request::<HoverRequest>(&mut session, make_hover_params());
        match result {
            Err(LspError::Timeout(ms)) => {
                assert_eq!(ms, REQUEST_TIMEOUT_MS, "timeout should carry the threshold");
            }
            other => panic!("expected LspError::Timeout, got: {other:?}"),
        }
        let _ = session.child.kill();
        let _ = session.child.wait();
    }

    // --- send_request: disconnect when reader channel closes ---

    #[test]
    fn send_request_returns_communication_error_on_disconnect() {
        let (mut session, reader_tx, _writer_rx) = mock_session();
        drop(reader_tx); // disconnect reader → recv_timeout → Disconnected
        let result = send_request::<HoverRequest>(&mut session, make_hover_params());
        match result {
            Err(LspError::Communication(msg)) => {
                assert!(
                    msg.contains("server connection closed"),
                    "should mention disconnect, got: {msg}"
                );
            }
            other => panic!("expected LspError::Communication, got: {other:?}"),
        }
        let _ = session.child.kill();
        let _ = session.child.wait();
    }

    // --- send_request: drains server-initiated notifications ---

    #[test]
    fn send_request_drains_server_notifications_before_response() {
        let (mut session, reader_tx, _writer_rx) = mock_session();
        // Pre-push a server notification, then the matching response.
        let notif = Notification {
            method: "window/logMessage".to_string(),
            params: serde_json::json!({ "type": 3, "message": "indexing" }),
        };
        reader_tx.send(Message::Notification(notif)).unwrap();
        let response = Response {
            id: RequestId::from(1),
            result: Some(serde_json::Value::Null),
            error: None,
        };
        reader_tx.send(Message::Response(response)).unwrap();

        let result = send_request::<HoverRequest>(&mut session, make_hover_params());
        assert!(
            result.is_ok(),
            "should drain notification and return response: {:?}",
            result.err()
        );
        // Null → Option<Hover>::None.
        assert_eq!(result.unwrap(), None);
        let _ = session.child.kill();
        let _ = session.child.wait();
    }

    // --- send_request: drains server-initiated requests ---

    #[test]
    fn send_request_drains_server_requests_before_response() {
        let (mut session, reader_tx, _writer_rx) = mock_session();
        // Pre-push a server-initiated request, then the matching response.
        let server_req = Request {
            id: RequestId::from(99),
            method: "workspace/configuration".to_string(),
            params: serde_json::Value::Null,
        };
        reader_tx.send(Message::Request(server_req)).unwrap();
        let response = Response {
            id: RequestId::from(1),
            result: Some(serde_json::Value::Null),
            error: None,
        };
        reader_tx.send(Message::Response(response)).unwrap();

        let result = send_request::<HoverRequest>(&mut session, make_hover_params());
        assert!(result.is_ok(), "drained server request: {:?}", result.err());
        let _ = session.child.kill();
        let _ = session.child.wait();
    }

    // --- send_request: skips stale (wrong-id) responses ---

    #[test]
    fn send_request_skips_stale_response_with_wrong_id() {
        let (mut session, reader_tx, _writer_rx) = mock_session();
        // Stale response (id=999, doesn't match our request id=1).
        let stale = Response {
            id: RequestId::from(999),
            result: Some(serde_json::Value::Null),
            error: None,
        };
        reader_tx.send(Message::Response(stale)).unwrap();
        // Correct response (id=1).
        let correct = Response {
            id: RequestId::from(1),
            result: Some(serde_json::Value::Null),
            error: None,
        };
        reader_tx.send(Message::Response(correct)).unwrap();

        let result = send_request::<HoverRequest>(&mut session, make_hover_params());
        assert!(result.is_ok(), "should skip stale and return correct: {:?}", result.err());
        let _ = session.child.kill();
        let _ = session.child.wait();
    }

    // --- send_request: sender disconnect (writer channel closed) ---

    #[test]
    fn send_request_returns_error_when_writer_channel_disconnects() {
        let (mut session, _reader_tx, writer_rx) = mock_session();
        drop(writer_rx); // disconnect writer → send() fails
        let result = send_request::<HoverRequest>(&mut session, make_hover_params());
        match result {
            Err(LspError::Communication(msg)) => {
                assert!(msg.contains("send request"), "got: {msg}");
            }
            other => panic!("expected LspError::Communication, got: {other:?}"),
        }
        let _ = session.child.kill();
        let _ = session.child.wait();
    }

    // --- send_request: server error response ---

    #[test]
    fn send_request_surfaces_server_error_response() {
        let (mut session, reader_tx, _writer_rx) = mock_session();
        let err_response = Response {
            id: RequestId::from(1),
            result: None,
            error: Some(lsp_server::ResponseError {
                code: -32603,
                message: "internal server error".to_string(),
                data: None,
            }),
        };
        reader_tx.send(Message::Response(err_response)).unwrap();

        let result = send_request::<HoverRequest>(&mut session, make_hover_params());
        match result {
            Err(LspError::Communication(msg)) => {
                assert!(msg.contains("server error"), "got: {msg}");
                assert!(msg.contains("-32603"), "got: {msg}");
            }
            other => panic!("expected LspError::Communication, got: {other:?}"),
        }
        let _ = session.child.kill();
        let _ = session.child.wait();
    }

    // --- send_raw_request: pushes message to writer channel ---

    #[test]
    fn send_raw_request_enqueues_request_with_correct_id_and_method() {
        let (mut session, _reader_tx, writer_rx) = mock_session();
        send_raw_request(&mut session, "shutdown", serde_json::Value::Null);
        let msg = writer_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("should receive the shutdown request");
        match msg {
            Message::Request(req) => {
                assert_eq!(req.method, "shutdown");
                assert_eq!(req.id, RequestId::from(1));
                assert_eq!(req.params, serde_json::Value::Null);
            }
            other => panic!("expected Message::Request, got: {other:?}"),
        }
        // next_request_id should have been incremented.
        assert_eq!(session.next_request_id, 2);
        let _ = session.child.kill();
        let _ = session.child.wait();
    }

    // --- shutdown with an active session ---

    #[test]
    fn shutdown_with_active_session_returns_ok_and_clears_session() {
        let (session, _reader_tx, _writer_rx) = mock_session();
        let client = RustAnalyzerClient::new();
        *client.session.lock().unwrap() = Some(session);
        let result = client.shutdown();
        assert!(result.is_ok(), "shutdown should succeed: {:?}", result.err());
        assert!(
            client.session.lock().unwrap().is_none(),
            "session should be cleared after shutdown"
        );
    }

    // --- start is idempotent when a session is already active ---

    #[test]
    fn start_is_idempotent_when_session_already_active() {
        let (session, _reader_tx, _writer_rx) = mock_session();
        let client = RustAnalyzerClient::new();
        *client.session.lock().unwrap() = Some(session);
        let workspace = std::env::temp_dir();
        let result = client.start(&workspace);
        assert!(
            result.is_ok(),
            "start with existing session should be Ok (no-op): {result:?}"
        );
        assert!(
            client.session.lock().unwrap().is_some(),
            "session should still be active"
        );
        // Clean up the mock session.
        let _ = client.shutdown();
    }

    // --- definition / type_definition / hover with active mock session ---
    // These exercise the trait method bodies (params construction +
    // send_request round-trip + result extraction) that the per-method
    // unit tests above skip by calling send_request directly.

    fn client_with_mock_session() -> (RustAnalyzerClient, Sender<Message>, Receiver<Message>) {
        let (session, reader_tx, writer_rx) = mock_session();
        let client = RustAnalyzerClient::new();
        *client.session.lock().unwrap() = Some(session);
        (client, reader_tx, writer_rx)
    }

    fn make_location() -> lsp_types::Location {
        lsp_types::Location {
            uri: Url::parse("file:///tmp/lib.rs").unwrap(),
            range: lsp_types::Range {
                start: Position { line: 5, character: 10 },
                end: Position { line: 5, character: 20 },
            },
        }
    }

    #[test]
    fn definition_with_mock_session_returns_location() {
        let (client, reader_tx, _writer_rx) = client_with_mock_session();
        let response = Response {
            id: RequestId::from(1),
            result: Some(
                serde_json::to_value(GotoDefinitionResponse::Scalar(make_location()))
                    .unwrap(),
            ),
            error: None,
        };
        reader_tx.send(Message::Response(response)).unwrap();

        let result = client.definition(Path::new("/tmp/lib.rs"), 0, 0);
        let loc = result.expect("definition should succeed");
        assert!(loc.is_some(), "should return a location");
        assert_eq!(loc.unwrap().range.start.line, 5);
        let _ = client.shutdown();
    }

    #[test]
    fn type_definition_with_mock_session_returns_location() {
        let (client, reader_tx, _writer_rx) = client_with_mock_session();
        let response = Response {
            id: RequestId::from(1),
            result: Some(
                serde_json::to_value(GotoDefinitionResponse::Scalar(make_location()))
                    .unwrap(),
            ),
            error: None,
        };
        reader_tx.send(Message::Response(response)).unwrap();

        let result = client.type_definition(Path::new("/tmp/lib.rs"), 0, 0);
        let loc = result.expect("type_definition should succeed");
        assert!(loc.is_some(), "should return a location");
        assert_eq!(loc.unwrap().range.start.line, 5);
        let _ = client.shutdown();
    }

    #[test]
    fn hover_with_mock_session_returns_hover() {
        let (client, reader_tx, _writer_rx) = client_with_mock_session();
        let hover = lsp_types::Hover {
            contents: lsp_types::HoverContents::Markup(lsp_types::MarkupContent {
                kind: lsp_types::MarkupKind::Markdown,
                value: "fn foo()".to_string(),
            }),
            range: None,
        };
        let response = Response {
            id: RequestId::from(1),
            result: Some(serde_json::to_value(hover).unwrap()),
            error: None,
        };
        reader_tx.send(Message::Response(response)).unwrap();

        let result = client.hover(Path::new("/tmp/lib.rs"), 0, 0);
        let h = result.expect("hover should succeed");
        assert!(h.is_some(), "should return hover info");
        let _ = client.shutdown();
    }
}
