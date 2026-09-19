// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Generic external LSP server client.
//!
//! All six external language-server adapters (gopls, pyright, clangd,
//! typescript-language-server, jdtls, fortls) previously carried
//! byte-identical copies of the same ~155-line client — only the default
//! binary name differed. The implementation now lives once in
//! [`ServerClient`], parameterized by an [`LspServerSpec`] associated const;
//! each adapter file is reduced to its spec plus a type alias, keeping the
//! public names (`GoplsClient` etc.) and per-language test modules stable.
//!
//! [`RustAnalyzerClient`](super::client::RustAnalyzerClient) is intentionally
//! NOT on this spec: it has server-specific initialization options.

use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use lsp_types::notification::{Exit, Notification as _};
use lsp_types::request::{GotoDefinition, GotoTypeDefinition, HoverRequest};
use lsp_types::{GotoDefinitionParams, HoverParams, PartialResultParams, WorkDoneProgressParams};

use super::references_cache::ReferencesCache;
use super::session::{self, Session};
use super::{LspError, LspProvider};

/// Per-server identity for [`ServerClient`].
pub trait LspServerSpec: Send + Sync + 'static {
    /// Default binary name on PATH (e.g. `"gopls"`).
    const DEFAULT_SERVER_PATH: &'static str;
}

/// Generic LSP client over an [`LspServerSpec`]: owns the child-process
/// session and implements [`LspProvider`] with the standard
/// initialize → request → shutdown protocol.
pub struct ServerClient<S: LspServerSpec> {
    /// Server binary path (defaults to [`LspServerSpec::DEFAULT_SERVER_PATH`]).
    pub(crate) server_path: PathBuf,
    /// Active child-process session (`None` before `start` / after `shutdown`).
    pub(crate) session: Mutex<Option<Session>>,
    /// TTL cache for `textDocument/references`.
    pub(crate) references_cache: ReferencesCache,
    _spec: PhantomData<S>,
}

impl<S: LspServerSpec> ServerClient<S> {
    /// Creates a client using the spec's default server path.
    #[must_use]
    pub fn new() -> Self {
        Self::with_server_path(PathBuf::from(S::DEFAULT_SERVER_PATH))
    }

    /// Creates a client with a custom server binary path.
    #[must_use]
    pub fn with_server_path(server_path: PathBuf) -> Self {
        Self {
            server_path,
            session: Mutex::new(None),
            references_cache: ReferencesCache::new(),
            _spec: PhantomData,
        }
    }

    /// Creates a client with a custom [`ReferencesCache`] — used by tests
    /// to inject a [`MockClock`](super::references_cache::MockClock)-backed
    /// cache for deterministic TTL verification.
    #[must_use]
    pub fn with_references_cache(server_path: PathBuf, references_cache: ReferencesCache) -> Self {
        Self {
            server_path,
            session: Mutex::new(None),
            references_cache,
            _spec: PhantomData,
        }
    }
}

impl<S: LspServerSpec> Default for ServerClient<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: LspServerSpec> LspProvider for ServerClient<S> {
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
            text_document_position_params: session::make_position_params(file, line, col)?,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        let resp = session::send_request::<GotoDefinition>(session, params)?;
        Ok(session::extract_first_location(resp))
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
        let params = GotoDefinitionParams {
            text_document_position_params: session::make_position_params(file, line, col)?,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        let resp = session::send_request::<GotoTypeDefinition>(session, params)?;
        Ok(session::extract_first_location(resp))
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
            text_document_position_params: session::make_position_params(file, line, col)?,
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        session::send_request::<HoverRequest>(session, params)
    }

    fn references(
        &self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<lsp_types::Location>, LspError> {
        session::references_impl(&self.session, &self.references_cache, file, line, col)
    }

    fn shutdown(&self) -> Result<(), LspError> {
        let mut guard = self.session.lock().expect("session mutex poisoned");
        let Some(mut session) = guard.take() else {
            return Ok(());
        };
        session::send_raw_request(&mut session, "shutdown", serde_json::Value::Null);
        let _ =
            session::send_notification(&session.connection, Exit::METHOD, &serde_json::Value::Null);
        let _ = session.child.wait();
        drop(session);
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::gopls::{GoplsClient, GoplsSpec};
    use crate::lsp::session::Session;
    use crate::lsp::{LspError, LspProvider};
    use std::path::{Path, PathBuf};

    use crate::lsp::session;
    use lsp_server::{Connection, Message, RequestId, Response};
    use lsp_types::request::HoverRequest;
    use lsp_types::{
        GotoDefinitionResponse, HoverParams, Position, TextDocumentIdentifier,
        TextDocumentPositionParams, Uri, WorkDoneProgressParams,
    };

    #[test]
    fn start_nonexistent_server_returns_error() {
        let client = GoplsClient::with_server_path(PathBuf::from("/nonexistent/path/to/gopls"));
        let dir = tempfile::tempdir().expect("tempdir");
        let result = client.start(dir.path());
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
        let dir = tempfile::tempdir().expect("tempdir");
        let _ = client.start(dir.path());
        assert!(client.shutdown().is_ok());
    }

    #[test]
    fn query_without_start_returns_communication_error() {
        let client = GoplsClient::new();
        assert!(matches!(
            client.definition(Path::new("/tmp/test.go"), 0, 0),
            Err(LspError::Communication(_))
        ));
        assert!(matches!(
            client.hover(Path::new("/tmp/test.go"), 0, 0),
            Err(LspError::Communication(_))
        ));
    }

    #[test]
    fn with_server_path_overrides_default() {
        let p = PathBuf::from("/custom/gopls");
        assert_eq!(GoplsClient::with_server_path(p.clone()).server_path, p);
    }

    fn mock_session() -> (
        Session,
        crossbeam_channel::Sender<Message>,
        crossbeam_channel::Receiver<Message>,
    ) {
        let child = std::process::Command::new("true")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let (wt, wr) = crossbeam_channel::bounded(16);
        let (rt, rr) = crossbeam_channel::bounded(16);
        (
            Session {
                child,
                connection: Connection {
                    sender: wt,
                    receiver: rr,
                },
                _reader_handle: std::thread::spawn(|| {}),
                _writer_handle: std::thread::spawn(|| {}),
                next_request_id: 1,
            },
            rt,
            wr,
        )
    }

    fn hp() -> HoverParams {
        HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: "file:///tmp/x.go".parse::<Uri>().unwrap(),
                },
                position: Position {
                    line: 0,
                    character: 0,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        }
    }

    #[test]
    fn send_request_timeout() {
        let (mut s, _rt, _wr) = mock_session();
        assert!(matches!(
            session::send_request::<HoverRequest>(&mut s, hp()),
            Err(LspError::Timeout(_))
        ));
        let _ = s.child.kill();
        let _ = s.child.wait();
    }

    #[test]
    fn shutdown_with_active_session() {
        let (s, _rt, _wr) = mock_session();
        let c = GoplsClient::new();
        *c.session.lock().unwrap() = Some(s);
        assert!(c.shutdown().is_ok());
        assert!(c.session.lock().unwrap().is_none());
    }

    fn loc() -> lsp_types::Location {
        lsp_types::Location {
            uri: "file:///tmp/test.go".parse::<Uri>().unwrap(),
            range: lsp_types::Range {
                start: Position {
                    line: 5,
                    character: 10,
                },
                end: Position {
                    line: 5,
                    character: 20,
                },
            },
        }
    }

    #[test]
    fn definition_with_mock() {
        let (s, rt, _wr) = mock_session();
        let c = GoplsClient::new();
        *c.session.lock().unwrap() = Some(s);
        rt.send(Message::Response(Response {
            id: RequestId::from(1),
            response_result: Ok(
                serde_json::to_value(GotoDefinitionResponse::Scalar(loc())).unwrap()
            ),
        }))
        .unwrap();
        assert_eq!(
            c.definition(Path::new("/tmp/test.go"), 0, 0)
                .unwrap()
                .unwrap()
                .range
                .start
                .line,
            5
        );
        let _ = c.shutdown();
    }

    #[test]
    fn hover_with_mock() {
        let (s, rt, _wr) = mock_session();
        let c = GoplsClient::new();
        *c.session.lock().unwrap() = Some(s);
        let hover = lsp_types::Hover {
            contents: lsp_types::HoverContents::Markup(lsp_types::MarkupContent {
                kind: lsp_types::MarkupKind::Markdown,
                value: "func foo() string".into(),
            }),
            range: None,
        };
        rt.send(Message::Response(Response {
            id: RequestId::from(1),
            response_result: Ok(serde_json::to_value(hover).unwrap()),
        }))
        .unwrap();
        assert!(c.hover(Path::new("/tmp/test.go"), 0, 0).unwrap().is_some());
        let _ = c.shutdown();
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

    /// The generic client honors its spec's default binary path (mechanism
    /// test on the representative `GoplsSpec`; each adapter file asserts its
    /// own const separately).
    #[test]
    fn server_client_uses_spec_default_path() {
        assert_eq!(
            ServerClient::<GoplsSpec>::new().server_path,
            PathBuf::from(GoplsSpec::DEFAULT_SERVER_PATH)
        );
    }
}
