// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! gopls LSP client for Go.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use lsp_types::request::{GotoDefinition, GotoTypeDefinition, HoverRequest};
use lsp_types::{GotoDefinitionParams, HoverParams, PartialResultParams, WorkDoneProgressParams};
use lsp_types::notification::{Exit, Notification as _};

use super::session::{self, Session};
use super::{LspError, LspProvider};

const DEFAULT_SERVER_PATH: &str = "gopls";

pub struct GoplsClient {
    server_path: PathBuf,
    session: Mutex<Option<Session>>,
}

impl GoplsClient {
    #[must_use]
    pub fn new() -> Self {
        Self::with_server_path(PathBuf::from(DEFAULT_SERVER_PATH))
    }

    #[must_use]
    pub fn with_server_path(server_path: PathBuf) -> Self {
        Self {
            server_path,
            session: Mutex::new(None),
        }
    }
}

impl Default for GoplsClient {
    fn default() -> Self {
        Self::new()
    }
}

impl LspProvider for GoplsClient {
    fn start(&self, workspace: &Path) -> Result<(), LspError> {
        let mut guard = self.session.lock().expect("session mutex poisoned");
        if guard.is_some() {
            return Ok(());
        }
        let (child, stdin, stdout) = session::spawn_server(&self.server_path, workspace, &[])?;
        let (connection, reader_handle, writer_handle) = session::spawn_transport(stdin, stdout);
        let mut session = Session {
            child,
            connection,
            _reader_handle: reader_handle,
            _writer_handle: writer_handle,
            next_request_id: 1,
        };
        session::initialize_session(&mut session, workspace)?;
        *guard = Some(session);
        Ok(())
    }

    fn definition(&self, file: &Path, line: u32, col: u32) -> Result<Option<lsp_types::Location>, LspError> {
        let mut guard = self.session.lock().expect("session mutex poisoned");
        let session = guard.as_mut().ok_or_else(|| LspError::Communication("LSP server not started".into()))?;
        let params = GotoDefinitionParams {
            text_document_position_params: session::make_position_params(file, line, col)?,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        let resp = session::send_request::<GotoDefinition>(session, params)?;
        Ok(session::extract_first_location(resp))
    }

    fn type_definition(&self, file: &Path, line: u32, col: u32) -> Result<Option<lsp_types::Location>, LspError> {
        let mut guard = self.session.lock().expect("session mutex poisoned");
        let session = guard.as_mut().ok_or_else(|| LspError::Communication("LSP server not started".into()))?;
        let params = GotoDefinitionParams {
            text_document_position_params: session::make_position_params(file, line, col)?,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        let resp = session::send_request::<GotoTypeDefinition>(session, params)?;
        Ok(session::extract_first_location(resp))
    }

    fn hover(&self, file: &Path, line: u32, col: u32) -> Result<Option<lsp_types::Hover>, LspError> {
        let mut guard = self.session.lock().expect("session mutex poisoned");
        let session = guard.as_mut().ok_or_else(|| LspError::Communication("LSP server not started".into()))?;
        let params = HoverParams {
            text_document_position_params: session::make_position_params(file, line, col)?,
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        session::send_request::<HoverRequest>(session, params)
    }

    fn shutdown(&self) -> Result<(), LspError> {
        let mut guard = self.session.lock().expect("session mutex poisoned");
        let Some(mut session) = guard.take() else {
            return Ok(());
        };
        session::send_raw_request(&mut session, "shutdown", serde_json::Value::Null);
        let _ = session::send_notification(&session.connection, Exit::METHOD, &serde_json::Value::Null);
        let _ = session.child.wait();
        drop(session);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::session;
    use std::path::PathBuf;
    use lsp_server::{Connection, Message, RequestId, Response};
    use lsp_types::{
        GotoDefinitionResponse, HoverParams, Position,
        TextDocumentIdentifier, TextDocumentPositionParams, Url, WorkDoneProgressParams,
    };
    use lsp_types::request::HoverRequest;

    #[test]
    fn start_nonexistent_server_returns_error() {
        let client = GoplsClient::with_server_path(PathBuf::from("/nonexistent/path/to/gopls"));
        let result = client.start(&std::env::temp_dir());
        match result {
            Err(LspError::ServerStart(msg)) => assert!(!msg.is_empty()),
            other => panic!("expected Err(LspError::ServerStart(_)), got: {other:?}"),
        }
    }

    #[test]
    fn shutdown_without_start_returns_ok() {
        assert!(GoplsClient::new().shutdown().is_ok());
    }

    #[test]
    fn shutdown_after_failed_start_returns_ok() {
        let client = GoplsClient::with_server_path(PathBuf::from("/nonexistent/path/to/gopls"));
        let _ = client.start(&std::env::temp_dir());
        assert!(client.shutdown().is_ok());
    }

    #[test]
    fn query_without_start_returns_communication_error() {
        let client = GoplsClient::new();
        assert!(matches!(client.definition(Path::new("/tmp/test.go"), 0, 0), Err(LspError::Communication(_))));
        assert!(matches!(client.hover(Path::new("/tmp/test.go"), 0, 0), Err(LspError::Communication(_))));
    }

    #[test]
    fn new_uses_default_server_path() {
        assert_eq!(GoplsClient::new().server_path, PathBuf::from(DEFAULT_SERVER_PATH));
    }

    #[test]
    fn with_server_path_overrides_default() {
        let p = PathBuf::from("/custom/gopls");
        assert_eq!(GoplsClient::with_server_path(p.clone()).server_path, p);
    }

    fn mock_session() -> (Session, crossbeam_channel::Sender<Message>, crossbeam_channel::Receiver<Message>) {
        let child = std::process::Command::new("true").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let (wt, wr) = crossbeam_channel::bounded(16);
        let (rt, rr) = crossbeam_channel::bounded(16);
        (Session { child, connection: Connection { sender: wt, receiver: rr }, _reader_handle: std::thread::spawn(|| {}), _writer_handle: std::thread::spawn(|| {}), next_request_id: 1 }, rt, wr)
    }

    fn hp() -> HoverParams { HoverParams { text_document_position_params: TextDocumentPositionParams { text_document: TextDocumentIdentifier { uri: Url::parse("file:///tmp/x.go").unwrap() }, position: Position { line: 0, character: 0 } }, work_done_progress_params: WorkDoneProgressParams::default() } }

    #[test]
    fn send_request_timeout() {
        let (mut s, _rt, _wr) = mock_session();
        assert!(matches!(session::send_request::<HoverRequest>(&mut s, hp()), Err(LspError::Timeout(_))));
        let _ = s.child.kill(); let _ = s.child.wait();
    }

    #[test]
    fn shutdown_with_active_session() {
        let (s, _rt, _wr) = mock_session();
        let c = GoplsClient::new();
        *c.session.lock().unwrap() = Some(s);
        assert!(c.shutdown().is_ok());
        assert!(c.session.lock().unwrap().is_none());
    }

    fn loc() -> lsp_types::Location { lsp_types::Location { uri: Url::parse("file:///tmp/test.go").unwrap(), range: lsp_types::Range { start: Position { line: 5, character: 10 }, end: Position { line: 5, character: 20 } } } }

    #[test]
    fn definition_with_mock() {
        let (s, rt, _wr) = mock_session();
        let c = GoplsClient::new();
        *c.session.lock().unwrap() = Some(s);
        rt.send(Message::Response(Response { id: RequestId::from(1), result: Some(serde_json::to_value(GotoDefinitionResponse::Scalar(loc())).unwrap()), error: None })).unwrap();
        assert_eq!(c.definition(Path::new("/tmp/test.go"), 0, 0).unwrap().unwrap().range.start.line, 5);
        let _ = c.shutdown();
    }

    #[test]
    fn hover_with_mock() {
        let (s, rt, _wr) = mock_session();
        let c = GoplsClient::new();
        *c.session.lock().unwrap() = Some(s);
        let hover = lsp_types::Hover { contents: lsp_types::HoverContents::Markup(lsp_types::MarkupContent { kind: lsp_types::MarkupKind::Markdown, value: "func foo() string".into() }), range: None };
        rt.send(Message::Response(Response { id: RequestId::from(1), result: Some(serde_json::to_value(hover).unwrap()), error: None })).unwrap();
        assert!(c.hover(Path::new("/tmp/test.go"), 0, 0).unwrap().is_some());
        let _ = c.shutdown();
    }

    #[test]
    #[ignore = "requires gopls on PATH; run with --ignored"]
    fn integration_start_shutdown() {
        if std::process::Command::new("gopls").arg("--version").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_err() { return; }
        let ws = tempfile::TempDir::new().unwrap();
        std::fs::write(ws.path().join("go.mod"), "module test\n\ngo 1.21\n").unwrap();
        std::fs::write(ws.path().join("test.go"), "package main\n\nfunc main() {}\n").unwrap();
        let c = GoplsClient::new();
        c.start(ws.path()).unwrap();
        c.shutdown().unwrap();
    }
}
