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
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    GotoDefinitionResponse, InitializeParams, InitializedParams, Position,
    TextDocumentIdentifier, TextDocumentPositionParams, Url, WorkspaceFolder,
};
use lsp_types::notification::{Initialized, Notification as _};
use lsp_types::request::Initialize;

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
) -> Result<(Child, ChildStdin, ChildStdout), LspError> {
    let mut child = Command::new(server_path)
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
pub(crate) fn send_request<R>(session: &mut Session, params: R::Params) -> Result<R::Result, LspError>
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
    if let Some(err) = resp.error {
        return Err(LspError::Communication(format!(
            "server error {}: {}",
            err.code, err.message
        )));
    }
    match resp.result {
        Some(value) => serde_json::from_value::<R::Result>(value)
            .map_err(|e| LspError::Communication(format!("decode response: {e}"))),
        None => serde_json::from_value::<R::Result>(serde_json::Value::Null)
            .map_err(|e| LspError::Communication(format!("decode null response: {e}"))),
    }
}

/// Convert a [`GotoDefinitionResponse`] into the first [`lsp_types::Location`].
pub(crate) fn extract_first_location(resp: Option<GotoDefinitionResponse>) -> Option<lsp_types::Location> {
    match resp? {
        GotoDefinitionResponse::Scalar(loc) => Some(loc),
        GotoDefinitionResponse::Array(locs) => locs.into_iter().next(),
        GotoDefinitionResponse::Link(links) => links.into_iter().next().map(|link| lsp_types::Location {
            uri: link.target_uri,
            range: link.target_range,
        }),
    }
}

/// Build [`TextDocumentPositionParams`] from a file path + 0-based line/col.
pub(crate) fn make_position_params(
    file: &Path,
    line: u32,
    col: u32,
) -> Result<TextDocumentPositionParams, LspError> {
    let uri = path_to_url(file)?;
    Ok(TextDocumentPositionParams {
        text_document: TextDocumentIdentifier { uri },
        position: Position {
            line,
            character: col,
        },
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
