// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `rust-analyzer` LSP client.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use lsp_types::notification::{Exit, Notification as _};
use lsp_types::request::{GotoDefinition, GotoTypeDefinition, HoverRequest};
use lsp_types::{GotoDefinitionParams, HoverParams, PartialResultParams, WorkDoneProgressParams};

use super::references_cache::ReferencesCache;
use super::session::{self, Session};
use super::{LspError, LspProvider};

const DEFAULT_SERVER_PATH: &str = "rust-analyzer";

pub struct RustAnalyzerClient {
    server_path: PathBuf,
    session: Mutex<Option<Session>>,
    references_cache: ReferencesCache,
}

impl RustAnalyzerClient {
    #[must_use]
    pub fn new() -> Self {
        Self::with_server_path(PathBuf::from(DEFAULT_SERVER_PATH))
    }

    #[must_use]
    pub fn with_server_path(server_path: PathBuf) -> Self {
        Self {
            server_path,
            session: Mutex::new(None),
            references_cache: ReferencesCache::new(),
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
        }
    }
}

impl Default for RustAnalyzerClient {
    fn default() -> Self {
        Self::new()
    }
}

impl LspProvider for RustAnalyzerClient {
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
    use crate::lsp::session;
    use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
    use lsp_types::request::{HoverRequest, Initialize};
    use lsp_types::{
        GotoDefinitionResponse, HoverParams, Position, TextDocumentIdentifier,
        TextDocumentPositionParams, Uri, WorkDoneProgressParams,
    };
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn start_nonexistent_server_returns_error() {
        let client = RustAnalyzerClient::with_server_path(PathBuf::from(
            "/nonexistent/path/to/rust-analyzer",
        ));
        let result = client.start(&std::env::temp_dir());
        match result {
            Err(LspError::ServerStart(msg)) => assert!(!msg.is_empty()),
            other => panic!("expected Err(LspError::ServerStart(_)), got: {other:?}"),
        }
    }

    #[test]
    fn shutdown_without_start_returns_ok() {
        let client = RustAnalyzerClient::new();
        assert!(client.shutdown().is_ok());
    }

    #[test]
    fn shutdown_after_failed_start_returns_ok() {
        let client = RustAnalyzerClient::with_server_path(PathBuf::from(
            "/nonexistent/path/to/rust-analyzer",
        ));
        let _ = client.start(&std::env::temp_dir());
        assert!(client.shutdown().is_ok());
    }

    #[test]
    fn query_without_start_returns_communication_error() {
        let client = RustAnalyzerClient::new();
        assert!(matches!(
            client.definition(Path::new("/tmp/lib.rs"), 0, 0),
            Err(LspError::Communication(_))
        ));
        assert!(matches!(
            client.type_definition(Path::new("/tmp/lib.rs"), 0, 0),
            Err(LspError::Communication(_))
        ));
        assert!(matches!(
            client.hover(Path::new("/tmp/lib.rs"), 0, 0),
            Err(LspError::Communication(_))
        ));
    }

    #[test]
    fn new_uses_default_server_path() {
        assert_eq!(
            RustAnalyzerClient::new().server_path,
            PathBuf::from(DEFAULT_SERVER_PATH)
        );
    }

    #[test]
    fn with_server_path_overrides_default() {
        let p = PathBuf::from("/custom/rust-analyzer");
        assert_eq!(
            RustAnalyzerClient::with_server_path(p.clone()).server_path,
            p
        );
    }

    #[test]
    fn default_impl_matches_new() {
        assert_eq!(
            RustAnalyzerClient::new().server_path,
            RustAnalyzerClient::default().server_path
        );
    }

    #[test]
    fn extract_first_location_from_scalar() {
        let uri = "file:///tmp/x.rs".parse::<Uri>().unwrap();
        let loc = lsp_types::Location {
            uri: uri.clone(),
            range: lsp_types::Range {
                start: Position {
                    line: 0,
                    character: 0,
                },
                end: Position {
                    line: 0,
                    character: 5,
                },
            },
        };
        assert_eq!(
            session::extract_first_location(Some(GotoDefinitionResponse::Scalar(loc.clone()))),
            Some(loc)
        );
    }

    #[test]
    fn extract_first_location_from_array_returns_first() {
        let uri = "file:///tmp/x.rs".parse::<Uri>().unwrap();
        let mk = |l: u32| lsp_types::Location {
            uri: uri.clone(),
            range: lsp_types::Range {
                start: Position {
                    line: l,
                    character: 0,
                },
                end: Position {
                    line: l,
                    character: 5,
                },
            },
        };
        assert_eq!(
            session::extract_first_location(Some(GotoDefinitionResponse::Array(vec![
                mk(1),
                mk(2)
            ])))
            .unwrap()
            .range
            .start
            .line,
            1
        );
    }

    #[test]
    fn extract_first_location_from_empty_returns_none() {
        assert_eq!(
            session::extract_first_location(Some(GotoDefinitionResponse::Array(vec![]))),
            None
        );
        assert_eq!(
            session::extract_first_location(Some(GotoDefinitionResponse::Link(vec![]))),
            None
        );
        assert_eq!(session::extract_first_location(None), None);
    }

    #[test]
    fn extract_first_location_from_link() {
        let uri = "file:///tmp/t.rs".parse::<Uri>().unwrap();
        let r = lsp_types::Range {
            start: Position {
                line: 10,
                character: 2,
            },
            end: Position {
                line: 10,
                character: 8,
            },
        };
        let link = lsp_types::LocationLink {
            origin_selection_range: None,
            target_uri: uri.clone(),
            target_range: r,
            target_selection_range: r,
        };
        let result =
            session::extract_first_location(Some(GotoDefinitionResponse::Link(vec![link])))
                .unwrap();
        assert_eq!(result.uri, uri);
    }

    #[test]
    fn decode_response_server_error() {
        let resp = Response {
            id: RequestId::from(1),
            response_result: Err(lsp_server::ResponseError {
                code: -32603,
                message: "err".into(),
                data: None,
            }),
        };
        assert!(matches!(
            session::decode_response::<Initialize>(resp),
            Err(LspError::Communication(_))
        ));
    }

    #[test]
    fn decode_response_absent_result_as_none() {
        let resp = Response {
            id: RequestId::from(1),
            response_result: Ok(serde_json::Value::Null),
        };
        assert_eq!(
            session::decode_response::<HoverRequest>(resp).unwrap(),
            None
        );
    }

    #[test]
    fn decode_response_type_mismatch() {
        let resp = Response {
            id: RequestId::from(1),
            response_result: Ok(serde_json::Value::String("bad".into())),
        };
        assert!(matches!(
            session::decode_response::<Initialize>(resp),
            Err(LspError::Communication(_))
        ));
    }

    #[test]
    fn send_notification_channel_disconnected() {
        use crossbeam_channel::bounded;
        let (s, r) = bounded::<Message>(1);
        drop(r);
        let (_dx, dr) = bounded::<Message>(1);
        assert!(matches!(
            session::send_notification(
                &Connection {
                    sender: s,
                    receiver: dr
                },
                "x",
                &serde_json::Value::Null
            ),
            Err(LspError::Communication(_))
        ));
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
                    uri: "file:///tmp/x.rs".parse::<Uri>().unwrap(),
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
    fn make_position_params() {
        let p = session::make_position_params(Path::new("/tmp/lib.rs"), 10, 20).unwrap();
        assert_eq!(p.text_document.uri.as_str(), "file:///tmp/lib.rs");
        assert_eq!(p.position.line, 10);
        assert_eq!(p.position.character, 20);
        assert!(session::make_position_params(Path::new("relative.rs"), 0, 0).is_err());
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
    fn send_request_disconnect() {
        let (mut s, rt, _wr) = mock_session();
        drop(rt);
        let r = session::send_request::<HoverRequest>(&mut s, hp());
        assert!(r
            .unwrap_err()
            .to_string()
            .contains("server connection closed"));
        let _ = s.child.kill();
        let _ = s.child.wait();
    }

    #[test]
    fn send_request_drains_notifications() {
        let (mut s, rt, _wr) = mock_session();
        rt.send(Message::Notification(Notification {
            method: "w/log".into(),
            params: serde_json::json!({}),
        }))
        .unwrap();
        rt.send(Message::Response(Response {
            id: RequestId::from(1),
            response_result: Ok(serde_json::Value::Null),
        }))
        .unwrap();
        assert!(session::send_request::<HoverRequest>(&mut s, hp())
            .unwrap()
            .is_none());
        let _ = s.child.kill();
        let _ = s.child.wait();
    }

    #[test]
    fn send_request_drains_server_requests() {
        let (mut s, rt, _wr) = mock_session();
        rt.send(Message::Request(Request {
            id: RequestId::from(99),
            method: "ws/config".into(),
            params: serde_json::Value::Null,
        }))
        .unwrap();
        rt.send(Message::Response(Response {
            id: RequestId::from(1),
            response_result: Ok(serde_json::Value::Null),
        }))
        .unwrap();
        assert!(session::send_request::<HoverRequest>(&mut s, hp()).is_ok());
        let _ = s.child.kill();
        let _ = s.child.wait();
    }

    #[test]
    fn send_request_skips_stale() {
        let (mut s, rt, _wr) = mock_session();
        rt.send(Message::Response(Response {
            id: RequestId::from(999),
            response_result: Ok(serde_json::Value::Null),
        }))
        .unwrap();
        rt.send(Message::Response(Response {
            id: RequestId::from(1),
            response_result: Ok(serde_json::Value::Null),
        }))
        .unwrap();
        assert!(session::send_request::<HoverRequest>(&mut s, hp()).is_ok());
        let _ = s.child.kill();
        let _ = s.child.wait();
    }

    #[test]
    fn send_request_writer_disconnect() {
        let (mut s, _rt, wr) = mock_session();
        drop(wr);
        assert!(session::send_request::<HoverRequest>(&mut s, hp())
            .unwrap_err()
            .to_string()
            .contains("send request"));
        let _ = s.child.kill();
        let _ = s.child.wait();
    }

    #[test]
    fn send_request_server_error() {
        let (mut s, rt, _wr) = mock_session();
        rt.send(Message::Response(Response {
            id: RequestId::from(1),
            response_result: Err(lsp_server::ResponseError {
                code: -32603,
                message: "err".into(),
                data: None,
            }),
        }))
        .unwrap();
        let r = session::send_request::<HoverRequest>(&mut s, hp());
        assert!(r.unwrap_err().to_string().contains("server error"));
        let _ = s.child.kill();
        let _ = s.child.wait();
    }

    #[test]
    fn send_raw_request_enqueues() {
        let (mut s, _rt, wr) = mock_session();
        session::send_raw_request(&mut s, "shutdown", serde_json::Value::Null);
        let msg = wr.recv_timeout(Duration::from_secs(1)).unwrap();
        match msg {
            Message::Request(req) => {
                assert_eq!(req.method, "shutdown");
                assert_eq!(req.id, RequestId::from(1));
            }
            _ => panic!(),
        }
        assert_eq!(s.next_request_id, 2);
        let _ = s.child.kill();
        let _ = s.child.wait();
    }

    #[test]
    fn shutdown_with_active_session() {
        let (s, _rt, _wr) = mock_session();
        let c = RustAnalyzerClient::new();
        *c.session.lock().unwrap() = Some(s);
        assert!(c.shutdown().is_ok());
        assert!(c.session.lock().unwrap().is_none());
    }

    #[test]
    fn start_idempotent() {
        let (s, _rt, _wr) = mock_session();
        let c = RustAnalyzerClient::new();
        *c.session.lock().unwrap() = Some(s);
        assert!(c.start(&std::env::temp_dir()).is_ok());
        assert!(c.session.lock().unwrap().is_some());
        let _ = c.shutdown();
    }

    fn client_with_mock() -> (
        RustAnalyzerClient,
        crossbeam_channel::Sender<Message>,
        crossbeam_channel::Receiver<Message>,
    ) {
        let (s, rt, wr) = mock_session();
        let c = RustAnalyzerClient::new();
        *c.session.lock().unwrap() = Some(s);
        (c, rt, wr)
    }

    /// Like [`client_with_mock`] but injects a custom [`ReferencesCache`]
    /// (typically backed by [`MockClock`](crate::lsp::references_cache::MockClock))
    /// so the 5-minute TTL can be fast-forwarded in tests.
    fn client_with_mock_and_cache(
        cache: crate::lsp::references_cache::ReferencesCache,
    ) -> (
        RustAnalyzerClient,
        crossbeam_channel::Sender<Message>,
        crossbeam_channel::Receiver<Message>,
    ) {
        let (s, rt, wr) = mock_session();
        let c =
            RustAnalyzerClient::with_references_cache(PathBuf::from(DEFAULT_SERVER_PATH), cache);
        *c.session.lock().unwrap() = Some(s);
        (c, rt, wr)
    }

    fn loc() -> lsp_types::Location {
        lsp_types::Location {
            uri: "file:///tmp/lib.rs".parse::<Uri>().unwrap(),
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
        let (c, rt, _wr) = client_with_mock();
        rt.send(Message::Response(Response {
            id: RequestId::from(1),
            response_result: Ok(
                serde_json::to_value(GotoDefinitionResponse::Scalar(loc())).unwrap()
            ),
        }))
        .unwrap();
        assert_eq!(
            c.definition(Path::new("/tmp/lib.rs"), 0, 0)
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
    fn type_definition_with_mock() {
        let (c, rt, _wr) = client_with_mock();
        rt.send(Message::Response(Response {
            id: RequestId::from(1),
            response_result: Ok(
                serde_json::to_value(GotoDefinitionResponse::Scalar(loc())).unwrap()
            ),
        }))
        .unwrap();
        let r = c.type_definition(Path::new("/tmp/lib.rs"), 0, 0).unwrap();
        assert!(r.is_some());
        let _ = c.shutdown();
    }

    #[test]
    fn hover_with_mock() {
        let (c, rt, _wr) = client_with_mock();
        let hover = lsp_types::Hover {
            contents: lsp_types::HoverContents::Markup(lsp_types::MarkupContent {
                kind: lsp_types::MarkupKind::Markdown,
                value: "fn foo()".into(),
            }),
            range: None,
        };
        rt.send(Message::Response(Response {
            id: RequestId::from(1),
            response_result: Ok(serde_json::to_value(hover).unwrap()),
        }))
        .unwrap();
        assert!(c.hover(Path::new("/tmp/lib.rs"), 0, 0).unwrap().is_some());
        let _ = c.shutdown();
    }

    #[test]
    #[ignore = "requires rust-analyzer on PATH; run with --ignored"]
    fn integration_start_shutdown() {
        if std::process::Command::new("rust-analyzer")
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
            ws.path().join("Cargo.toml"),
            "[package]\nname = \"d\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(ws.path().join("src")).unwrap();
        std::fs::write(ws.path().join("src/main.rs"), "fn main() {}").unwrap();
        let c = RustAnalyzerClient::new();
        c.start(ws.path()).unwrap();
        c.shutdown().unwrap();
    }

    #[test]
    #[ignore = "requires rust-analyzer on PATH; run with --ignored"]
    fn integration_hover() {
        if std::process::Command::new("rust-analyzer")
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
            ws.path().join("Cargo.toml"),
            "[package]\nname = \"d\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(ws.path().join("src")).unwrap();
        std::fs::write(
            ws.path().join("src/main.rs"),
            "fn add(a: i32, b: i32) -> i32 { a + b }\nfn main() { let _ = add(1, 2); }\n",
        )
        .unwrap();
        let c = RustAnalyzerClient::new();
        c.start(ws.path()).unwrap();
        assert!(c.hover(&ws.path().join("src/main.rs"), 0, 4).is_ok());
        c.shutdown().unwrap();
    }

    // ---- C9: references method (T170 / T171a) ----

    /// T170 — `references` must return every impl-site location the
    /// `rust-analyzer` server reports for a trait method.
    ///
    /// Scenario: user invokes references on `fmt` declared in
    /// `trait Display { fn fmt(&self, ...) }`. rust-analyzer responds with
    /// three `impl Display for X` locations (Foo/Bar/Baz). The call must
    /// surface all three without filtering or truncation.
    #[test]
    fn test_rust_analyzer_references_returns_trait_implementations() {
        let (c, rt, _wr) = client_with_mock();

        // Simulate rust-analyzer responding to `textDocument/references`
        // for `fmt` on `trait Display` — return three impl sites.
        let uri = "file:///tmp/display.rs".parse::<Uri>().unwrap();
        let mk_loc = |line: u32| lsp_types::Location {
            uri: uri.clone(),
            range: lsp_types::Range {
                start: Position { line, character: 0 },
                end: Position {
                    line,
                    character: 10,
                },
            },
        };
        let impl_sites = vec![mk_loc(10), mk_loc(20), mk_loc(30)];
        rt.send(Message::Response(Response {
            id: RequestId::from(1),
            response_result: Ok(serde_json::to_value(&impl_sites).unwrap()),
        }))
        .unwrap();

        // `fmt` is declared at line 5 (0-based), col 7 — position is irrelevant
        // for the mock, but realistic values keep the test readable.
        let result = c
            .references(Path::new("/tmp/display.rs"), 5, 7)
            .expect("references should return Ok when the server responds with a location array");

        assert_eq!(
            result.len(),
            3,
            "must return all three impl Display for X fmt sites, got {result:?}"
        );
        assert_eq!(result[0].range.start.line, 10, "Foo impl at line 10");
        assert_eq!(result[1].range.start.line, 20, "Bar impl at line 20");
        assert_eq!(result[2].range.start.line, 30, "Baz impl at line 30");

        let _ = c.shutdown();
    }

    /// T171a — `references` results must be cached for 5 minutes keyed by
    /// `(uri, line, column)`. Second call within the TTL window must not
    /// hit the LSP server; after the TTL elapses the cache must invalidate
    /// and the next call must dispatch a fresh request.
    ///
    /// Uses `MockClock` injection (no real `thread::sleep`) so the test
    /// runs in milliseconds rather than 5 wall-clock minutes.
    #[test]
    fn test_references_result_cached_for_5_minutes() {
        use crate::lsp::references_cache::{MockClock, ReferencesCache};
        use std::sync::Arc;
        use std::time::Duration;

        let mock_clock = Arc::new(MockClock::new());
        let cache = ReferencesCache::with_clock(
            mock_clock.clone(),
            Duration::from_secs(300),
            ReferencesCache::DEFAULT_CAPACITY,
        );
        let (c, rt, _wr) = client_with_mock_and_cache(cache);

        let uri = "file:///tmp/lib.rs".parse::<Uri>().unwrap();
        let loc_one = lsp_types::Location {
            uri,
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
        };

        // First call: cold cache, server must be hit. Pre-stage the response.
        rt.send(Message::Response(Response {
            id: RequestId::from(1),
            response_result: Ok(serde_json::to_value(vec![loc_one.clone()]).unwrap()),
        }))
        .unwrap();
        let r1 = c
            .references(Path::new("/tmp/lib.rs"), 5, 10)
            .expect("first call must succeed");
        assert_eq!(r1.len(), 1, "first call returns the single cached loc");
        let id_after_first = {
            let guard = c.session.lock().expect("session mutex poisoned");
            guard
                .as_ref()
                .expect("session present after first call")
                .next_request_id
        };
        assert_eq!(
            id_after_first, 2,
            "exactly one LSP request dispatched on first call (id 1->2)"
        );

        // Second call: identical (uri, line, col), within TTL. Cache must hit;
        // no LSP request dispatched. We do NOT pre-stage a response — if the
        // client hits the server, `recv_timeout` would block until timeout.
        let r2 = c
            .references(Path::new("/tmp/lib.rs"), 5, 10)
            .expect("second call within TTL must succeed via cache");
        assert_eq!(r2.len(), 1, "cached call returns the same single loc");
        let id_after_second = {
            let guard = c.session.lock().expect("session mutex poisoned");
            guard
                .as_ref()
                .expect("session still present")
                .next_request_id
        };
        assert_eq!(
            id_after_second, 2,
            "no new LSP request dispatched on cached call (id still 2)"
        );

        // Advance the mock clock past the 5-minute TTL.
        mock_clock.advance(Duration::from_secs(301));

        // Third call: cache expired, server must be hit again. Pre-stage the
        // response with the next request id (the client will dispatch id=2,
        // so the server response id must be 2).
        rt.send(Message::Response(Response {
            id: RequestId::from(2),
            response_result: Ok(serde_json::to_value(vec![loc_one]).unwrap()),
        }))
        .unwrap();
        let r3 = c
            .references(Path::new("/tmp/lib.rs"), 5, 10)
            .expect("third call after TTL expiry must succeed via fresh request");
        assert_eq!(r3.len(), 1, "post-expiry call returns the fresh loc");
        let id_after_third = {
            let guard = c.session.lock().expect("session mutex poisoned");
            guard
                .as_ref()
                .expect("session still present after third call")
                .next_request_id
        };
        assert_eq!(
            id_after_third, 3,
            "exactly one new LSP request dispatched on post-expiry call (id 2->3)"
        );

        let _ = c.shutdown();
    }
}
