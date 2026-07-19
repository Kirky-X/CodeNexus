// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Shared LSP session and transport helpers.
//!
//! Extracted from `client::RustAnalyzerClient` so that language-specific
//! clients (RustAnalyzerClient, PyrightClient, GoplsClient, …) reuse the
//! same subprocess-spawning, JSON-RPC-framing, request/response machinery
//! without duplicating ~200 lines per language.

use std::io::BufReader;
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::str::FromStr;
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{Initialized, Notification as _};
use lsp_types::request::{Initialize, References};
use lsp_types::{
    GotoDefinitionResponse, InitializeParams, InitializedParams, PartialResultParams, Position,
    ReferenceContext, ReferenceParams, TextDocumentIdentifier, TextDocumentPositionParams, Uri,
    WorkDoneProgressParams, WorkspaceFolder,
};

use super::references_cache::{CacheKey, ReferencesCache};
use super::{LspError, REQUEST_TIMEOUT_MS};

/// Active LSP session — populated by `start`, drained by `shutdown`.
pub(crate) struct Session {
    pub(crate) child: Child,
    pub(crate) connection: Connection,
    pub(crate) _reader_handle: JoinHandle<()>,
    pub(crate) _writer_handle: JoinHandle<()>,
    pub(crate) next_request_id: i32,
}

/// Wire a subprocess's stdin/stdout into an [`lsp_server::Connection`].
pub(crate) fn spawn_transport(
    stdin: ChildStdin,
    stdout: ChildStdout,
) -> (Connection, JoinHandle<()>, JoinHandle<()>) {
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
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
        })
        .expect("spawn lsp reader thread");

    let connection = Connection {
        sender: writer_tx,
        receiver: reader_rx,
    };
    (connection, reader_handle, writer_handle)
}

/// Spawn the LSP subprocess and return child + piped stdin/stdout.
pub(crate) fn spawn_server(
    server_path: &Path,
    workspace: &Path,
    args: &[&str],
) -> Result<(Child, ChildStdin, ChildStdout), LspError> {
    let mut child = Command::new(server_path)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .current_dir(workspace)
        .spawn()
        .map_err(|e| LspError::ServerStart(e.to_string()))?;

    let stdin = child.stdin.take().expect("stdin was Stdio::piped");
    let stdout = child.stdout.take().expect("stdout was Stdio::piped");
    Ok((child, stdin, stdout))
}

/// Perform the LSP `initialize` / `initialized` handshake.
pub(crate) fn initialize_session(session: &mut Session, workspace: &Path) -> Result<(), LspError> {
    let root_uri = path_to_uri(workspace)?;
    let init_params = InitializeParams {
        process_id: Some(std::process::id()),
        workspace_folders: Some(vec![WorkspaceFolder {
            uri: root_uri,
            name: workspace
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "workspace".to_string()),
        }]),
        capabilities: lsp_types::ClientCapabilities::default(),
        ..Default::default()
    };
    let _init_result = send_request::<Initialize>(session, init_params)?;
    send_notification(
        &session.connection,
        Initialized::METHOD,
        &InitializedParams {},
    )?;
    Ok(())
}

/// Send a typed LSP request and await its typed response.
pub(crate) fn send_request<R>(
    session: &mut Session,
    params: R::Params,
) -> Result<R::Result, LspError>
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
                    continue;
                }
                return decode_response::<R>(resp);
            }
            Message::Notification(_) | Message::Request(_) => continue,
        }
    }
}

/// Send a raw (untyped) request — used for the `shutdown` handshake.
pub(crate) fn send_raw_request(session: &mut Session, method: &str, params: serde_json::Value) {
    let id = session.next_request_id;
    session.next_request_id += 1;
    let request = Request {
        id: RequestId::from(id),
        method: method.to_string(),
        params,
    };
    let _ = session.connection.sender.send(Message::Request(request));
}

/// Send an LSP notification (no response expected).
pub(crate) fn send_notification<P: serde::Serialize>(
    conn: &Connection,
    method: &str,
    params: &P,
) -> Result<(), LspError> {
    let params_value =
        serde_json::to_value(params).map_err(|e| LspError::Communication(e.to_string()))?;
    let notif = Notification {
        method: method.to_string(),
        params: params_value,
    };
    conn.sender
        .send(Message::Notification(notif))
        .map_err(|e| LspError::Communication(format!("send notification: {e}")))
}

/// Decode a JSON-RPC [`Response`] into the typed result of `R`.
pub(crate) fn decode_response<R>(resp: Response) -> Result<R::Result, LspError>
where
    R: lsp_types::request::Request,
    R::Result: serde::de::DeserializeOwned,
{
    match resp.response_result {
        Err(error) => Err(LspError::Communication(format!(
            "server error {}: {}",
            error.code, error.message
        ))),
        Ok(result) => serde_json::from_value::<R::Result>(result)
            .map_err(|e| LspError::Communication(format!("decode response: {e}"))),
    }
}

/// Convert a [`GotoDefinitionResponse`] into the first [`lsp_types::Location`].
pub(crate) fn extract_first_location(
    resp: Option<GotoDefinitionResponse>,
) -> Option<lsp_types::Location> {
    match resp? {
        GotoDefinitionResponse::Scalar(loc) => Some(loc),
        GotoDefinitionResponse::Array(locs) => locs.into_iter().next(),
        GotoDefinitionResponse::Link(links) => {
            links.into_iter().next().map(|link| lsp_types::Location {
                uri: link.target_uri,
                range: link.target_range,
            })
        }
    }
}

/// Send `textDocument/references` and return every location the server
/// reports. `include_declaration` is `false` (C9 R-lsp-003: callers want
/// impl/call sites, not the declaration itself).
///
/// Returns `Ok(Vec::new())` when the server responds with `null` (no
/// references) — matches the LSP spec where `Option<Vec<Location>>`
/// semantically maps to an empty vec for callers that don't care about
/// the "server responded but had nothing" vs "server timed out"
/// distinction.
pub(crate) fn send_references_request(
    session: &mut Session,
    pos_params: TextDocumentPositionParams,
) -> Result<Vec<lsp_types::Location>, LspError> {
    let params = ReferenceParams {
        text_document_position: pos_params,
        context: ReferenceContext {
            include_declaration: false,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    let resp = send_request::<References>(session, params)?;
    Ok(resp.unwrap_or_default())
}

/// Shared `textDocument/references` implementation with cache lookup.
///
/// Encapsulates the cache-check → dispatch → cache-populate sequence used
/// by `RustAnalyzerClient`, `PyrightClient`, `ClangdClient` (C9 R-lsp-002).
/// Extracted to avoid triplicating the same ~22 lines across three client
/// structs — `definition`/`hover`/`type_definition` still duplicate per
/// client (pre-existing pattern, out of C9 scope to refactor).
///
/// # Flow
///
/// 1. Build [`TextDocumentPositionParams`] from `(file, line, col)`.
/// 2. Build [`CacheKey`] from the resulting URI.
/// 3. Check `cache` — return immediately on hit (within 5-min TTL).
/// 4. On miss, lock `session`, dispatch `textDocument/references`.
/// 5. Insert result into `cache` for subsequent calls.
///
/// # Errors
///
/// - [`LspError::Communication`] if the session is `None` (server not started).
/// - Propagates any [`LspError`] from [`send_references_request`].
pub(crate) fn references_impl(
    session: &Mutex<Option<Session>>,
    cache: &ReferencesCache,
    file: &Path,
    line: u32,
    col: u32,
) -> Result<Vec<lsp_types::Location>, LspError> {
    let pos_params = make_position_params(file, line, col)?;
    let cache_key = CacheKey::new(pos_params.text_document.uri.as_str().to_owned(), line, col);

    if let Some(cached) = cache.get(&cache_key) {
        return Ok(cached);
    }

    let mut guard = session.lock().expect("session mutex poisoned");
    let session = guard
        .as_mut()
        .ok_or_else(|| LspError::Communication("LSP server not started".into()))?;
    let locations = send_references_request(session, pos_params)?;

    cache.insert(cache_key, locations.clone());

    Ok(locations)
}

/// Build [`TextDocumentPositionParams`] from a file path + 0-based line/col.
pub(crate) fn make_position_params(
    file: &Path,
    line: u32,
    col: u32,
) -> Result<TextDocumentPositionParams, LspError> {
    let uri = path_to_uri(file)?;
    Ok(TextDocumentPositionParams {
        text_document: TextDocumentIdentifier { uri },
        position: Position {
            line,
            character: col,
        },
    })
}

/// Convert a filesystem path to a `file://` [`Uri`].
///
/// Uses [`url::Url::from_file_path`] for correct percent-encoding of
/// special characters, then re-parses the resulting URI string into
/// [`lsp_types::Uri`] (the lsp-types 0.97 newtype around `fluent_uri`).
fn path_to_uri(path: &Path) -> Result<Uri, LspError> {
    let url = url::Url::from_file_path(path).map_err(|_| {
        LspError::Communication(format!(
            "path is not absolute or cannot be encoded as a file URL: {}",
            path.display()
        ))
    })?;
    Uri::from_str(url.as_str())
        .map_err(|e| LspError::Communication(format!("failed to parse file URI '{url}': {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::request::Shutdown;

    fn make_test_session() -> (Session, Sender<Message>, Receiver<Message>) {
        let (writer_tx, writer_rx) = bounded::<Message>(16);
        let (reader_tx, reader_rx) = bounded::<Message>(16);
        let connection = Connection {
            sender: writer_tx,
            receiver: reader_rx,
        };
        let child = Command::new("true").spawn().expect("spawn");
        let session = Session {
            child,
            connection,
            _reader_handle: thread::spawn(|| {}),
            _writer_handle: thread::spawn(|| {}),
            next_request_id: 0,
        };
        (session, reader_tx, writer_rx)
    }

    #[test]
    fn decode_response_returns_error_for_server_error() {
        let resp = Response::new_err(RequestId::from(0), 1, "test error".to_string());
        let result: Result<(), LspError> = decode_response::<Shutdown>(resp);
        let err = result.expect_err("should return error");
        match err {
            LspError::Communication(msg) => {
                assert!(msg.contains("server error 1"), "msg: {msg}");
                assert!(msg.contains("test error"), "msg: {msg}");
            }
            other => panic!("expected Communication, got: {other:?}"),
        }
    }

    #[test]
    fn send_notification_succeeds() {
        let (tx, rx) = bounded::<Message>(16);
        let (_dummy_tx, dummy_rx) = bounded::<Message>(1);
        let conn = Connection {
            sender: tx,
            receiver: dummy_rx,
        };
        send_notification(&conn, "test/method", &"params").expect("should succeed");
        let msg = rx.recv().expect("should receive");
        match msg {
            Message::Notification(n) => assert_eq!(n.method, "test/method"),
            other => panic!("expected Notification, got: {other:?}"),
        }
    }

    #[test]
    fn send_notification_returns_error_on_closed_channel() {
        let (tx, rx) = bounded::<Message>(16);
        drop(rx);
        let (_dummy_tx, dummy_rx) = bounded::<Message>(1);
        let conn = Connection {
            sender: tx,
            receiver: dummy_rx,
        };
        let err = send_notification(&conn, "test", &"x").expect_err("should fail");
        assert!(matches!(err, LspError::Communication(_)));
    }

    #[test]
    fn send_request_returns_response() {
        let (mut session, reader_tx, writer_rx) = make_test_session();
        thread::spawn(move || {
            let req = writer_rx.recv().expect("recv");
            if let Message::Request(request) = req {
                let resp = Response::new_ok(request.id, serde_json::Value::Null);
                reader_tx.send(Message::Response(resp)).expect("send");
            }
        });
        let result = send_request::<Shutdown>(&mut session, ());
        assert!(result.is_ok(), "should return Ok: {result:?}");
    }

    #[test]
    fn send_request_skips_response_with_wrong_id() {
        let (mut session, reader_tx, writer_rx) = make_test_session();
        thread::spawn(move || {
            let req = writer_rx.recv().expect("recv");
            if let Message::Request(request) = req {
                let wrong = Response::new_ok(RequestId::from(999), serde_json::Value::Null);
                reader_tx.send(Message::Response(wrong)).expect("send");
                let correct = Response::new_ok(request.id, serde_json::Value::Null);
                reader_tx.send(Message::Response(correct)).expect("send");
            }
        });
        let result = send_request::<Shutdown>(&mut session, ());
        assert!(result.is_ok(), "should skip wrong id: {result:?}");
    }

    #[test]
    fn send_request_skips_notifications() {
        let (mut session, reader_tx, writer_rx) = make_test_session();
        thread::spawn(move || {
            let req = writer_rx.recv().expect("recv");
            if let Message::Request(request) = req {
                let notif = Notification::new("test".to_string(), serde_json::Value::Null);
                reader_tx.send(Message::Notification(notif)).expect("send");
                let resp = Response::new_ok(request.id, serde_json::Value::Null);
                reader_tx.send(Message::Response(resp)).expect("send");
            }
        });
        let result = send_request::<Shutdown>(&mut session, ());
        assert!(result.is_ok(), "should skip notification: {result:?}");
    }

    #[test]
    fn send_request_returns_error_on_disconnect() {
        let (mut session, reader_tx, _writer_rx) = make_test_session();
        drop(reader_tx);
        let err = send_request::<Shutdown>(&mut session, ()).expect_err("should fail");
        match err {
            LspError::Communication(msg) => assert!(msg.contains("closed"), "msg: {msg}"),
            other => panic!("expected Communication, got: {other:?}"),
        }
    }

    #[test]
    fn spawn_server_succeeds_with_cat() {
        let workspace = std::env::temp_dir();
        let result = spawn_server(Path::new("cat"), &workspace, &[]);
        assert!(result.is_ok(), "spawn_server should succeed: {result:?}");
        let (mut child, stdin, stdout) = result.unwrap();
        drop(stdin);
        drop(stdout);
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn spawn_server_fails_with_nonexistent_binary() {
        let workspace = std::env::temp_dir();
        let err = spawn_server(Path::new("/nonexistent/binary/path"), &workspace, &[])
            .expect_err("should fail");
        assert!(matches!(err, LspError::ServerStart(_)));
    }

    #[test]
    fn spawn_transport_round_trips_message() {
        let mut child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn cat");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let (conn, _reader_handle, _writer_handle) = spawn_transport(stdin, stdout);
        let notif = Notification::new("test/method".to_string(), serde_json::Value::Null);
        conn.sender
            .send(Message::Notification(notif))
            .expect("send");
        let msg = conn
            .receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("should receive echo");
        assert!(matches!(msg, Message::Notification(_)));
        drop(conn);
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn initialize_session_completes_handshake() {
        let (mut session, reader_tx, writer_rx) = make_test_session();
        thread::spawn(move || {
            if let Ok(Message::Request(request)) = writer_rx.recv_timeout(Duration::from_secs(2)) {
                let init_result = serde_json::json!({ "capabilities": {} });
                let resp = Response::new_ok(request.id, init_result);
                reader_tx.send(Message::Response(resp)).expect("send");
            }
            // Keep writer_rx alive briefly so send_notification doesn't fail
            thread::sleep(Duration::from_millis(500));
        });
        let workspace = std::env::current_dir().expect("cwd");
        let result = initialize_session(&mut session, &workspace);
        assert!(
            result.is_ok(),
            "initialize_session should succeed: {result:?}"
        );
    }
}
