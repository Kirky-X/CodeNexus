// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! End-to-end `codenexus hub` tests against a live (in-process) HTTP registry.
//!
//! These drive the compiled binary — the same path a user takes — through the
//! documented protocol-v1 endpoints:
//! `POST /artifacts`, `GET /artifacts/{name}/latest`,
//! `GET /artifacts?project=<name>`. The registry is a minimal HTTP/1.1 server
//! on `127.0.0.1:0`, so the tests need no external service.
#![cfg(feature = "hub")]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Returns the codenexus binary path.
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_codenexus")
}

/// Runs `codenexus <args>` and returns (exit_code, stdout, stderr).
fn run(args: &[&str]) -> (i32, String, String) {
    let output = Command::new(binary())
        .args(args)
        .env("CODENEXUS_LANG", "en")
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn codenexus: {e}"));
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// Registry state shared with the server thread.
#[derive(Default)]
struct RegistryState {
    /// Last body uploaded via `POST /artifacts`.
    stored: Mutex<Option<Vec<u8>>>,
    /// Value of the last `X-Codenexus-Project` request header.
    project_header: Mutex<Option<String>>,
    /// Last `Authorization` request header.
    auth_header: Mutex<Option<String>>,
    /// Query string of the last `GET /artifacts` request.
    last_query: Mutex<Option<String>>,
}

/// Minimal registry server: binds an ephemeral port and serves the three
/// protocol-v1 endpoints until `shutdown` is set.
fn spawn_registry() -> (
    String,
    Arc<RegistryState>,
    Arc<AtomicBool>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock registry");
    let addr = listener.local_addr().expect("local addr");
    let state = Arc::new(RegistryState::default());
    let shutdown = Arc::new(AtomicBool::new(false));
    let thread_state = Arc::clone(&state);
    let thread_shutdown = Arc::clone(&shutdown);
    let handle = thread::spawn(move || {
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        while !thread_shutdown.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => serve_request(&mut stream, &thread_state),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(_) => break,
            }
        }
    });
    (format!("http://{addr}"), state, shutdown, handle)
}

/// Reads one HTTP/1.1 request and writes one response.
fn serve_request(stream: &mut TcpStream, state: &RegistryState) {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
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
            Ok(0) | Err(_) => break,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
        }
    }
    let header_value = |name: &str| {
        headers.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case(name).then(|| v.trim().to_string())
        })
    };
    *state.project_header.lock().unwrap() = header_value("x-codenexus-project");
    *state.auth_header.lock().unwrap() = header_value("authorization");

    let (status, content_type, payload): (&str, &str, Vec<u8>) =
        if request_line.starts_with("POST /artifacts ") {
            *state.stored.lock().unwrap() = Some(body);
            (
                "200 OK",
                "application/json",
                br#"{"id":"art_1","url":"/artifacts/art_1"}"#.to_vec(),
            )
        } else if let Some(query) = request_line
            .strip_prefix("GET /artifacts?")
            .and_then(|rest| rest.split(' ').next())
        {
            *state.last_query.lock().unwrap() = Some(query.to_string());
            (
                "200 OK",
                "application/json",
                br#"[{"id":"art_1","name":"hubproj"}]"#.to_vec(),
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

/// Parses the single-line JSON receipt a `hub` command prints on stdout.
fn parse_receipt(stdout: &str) -> serde_json::Value {
    let line = stdout
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON receipt on stdout, got: {stdout:?}"));
    serde_json::from_str(line).unwrap_or_else(|e| panic!("receipt is not JSON ({e}): {line}"))
}

/// Creates an indexable repo + a fresh DB inside `dir`.
fn seed_project(dir: &std::path::Path, name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let repo = dir.join("repo");
    std::fs::create_dir_all(repo.join("src")).expect("create repo");
    std::fs::write(repo.join("src/main.rs"), "fn main() {}\n").expect("write source");
    let db = dir.join(format!("{name}.lbug"));
    let (code, stdout, stderr) = run(&[
        "index",
        "--path",
        repo.to_str().unwrap(),
        "--name",
        name,
        "--db",
        db.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "index failed: {stderr}\n{stdout}");
    (repo, db)
}

/// `push` → `list` → `pull` against a live registry, then verify the pulled
/// artifact imported into a second database.
#[test]
fn hub_push_pull_list_round_trip_against_http_registry() {
    let (base, registry, shutdown, server) = spawn_registry();
    let dir = tempfile::TempDir::new().expect("tempdir");
    let (_repo, db) = seed_project(dir.path(), "hubproj");
    let db = db.to_str().unwrap().to_string();

    // --- push: export the project and upload it ---
    let (code, stdout, stderr) = run(&[
        "hub",
        "--action",
        "push",
        "--url",
        &base,
        "--project",
        "hubproj",
        "--token",
        "secret",
        "--db",
        &db,
    ]);
    assert_eq!(code, 0, "push failed: {stderr}\n{stdout}");
    let receipt = parse_receipt(&stdout);
    for key in ["action", "url", "artifact", "project", "outcome", "details"] {
        assert!(
            receipt.get(key).is_some(),
            "receipt must carry `{key}`: {receipt}"
        );
    }
    assert_eq!(receipt["action"], "push");
    assert_eq!(receipt["url"], base);
    assert_eq!(receipt["project"], "hubproj");
    assert_eq!(receipt["outcome"], "ok");
    assert_eq!(
        receipt["details"]["id"], "art_1",
        "push details must carry the registry response: {}",
        receipt["details"]
    );
    assert_eq!(
        registry.project_header.lock().unwrap().as_deref(),
        Some("hubproj"),
        "push must send X-Codenexus-Project"
    );
    assert_eq!(
        registry.auth_header.lock().unwrap().as_deref(),
        Some("Bearer secret"),
        "push must send the bearer token"
    );
    assert!(
        registry
            .stored
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|b| !b.is_empty()),
        "push must upload a non-empty .cnxp body"
    );

    // --- list: filter by project ---
    let (code, stdout, stderr) = run(&[
        "hub",
        "--action",
        "list",
        "--url",
        &base,
        "--project",
        "hubproj",
        "--db",
        &db,
    ]);
    assert_eq!(code, 0, "list failed: {stderr}\n{stdout}");
    let receipt = parse_receipt(&stdout);
    assert_eq!(receipt["action"], "list");
    assert_eq!(receipt["project"], "hubproj");
    assert_eq!(receipt["outcome"], "ok");
    assert_eq!(
        receipt["details"].as_array().map(Vec::len),
        Some(1),
        "list details must be the registry's array: {}",
        receipt["details"]
    );
    assert_eq!(
        registry.last_query.lock().unwrap().as_deref(),
        Some("project=hubproj"),
        "list must filter by project"
    );

    // --- pull: download the latest artifact and import it (digest + size checked) ---
    let db2 = dir.path().join("pulled.lbug");
    let (code, stdout, stderr) = run(&[
        "hub",
        "--action",
        "pull",
        "--url",
        &base,
        "--name",
        "hubproj",
        "--db",
        db2.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "pull failed: {stderr}\n{stdout}");
    let receipt = parse_receipt(&stdout);
    assert_eq!(receipt["action"], "pull");
    assert_eq!(receipt["artifact"], "hubproj");
    assert_eq!(receipt["outcome"], "ok");
    assert!(
        receipt["details"]["db_size"].as_u64().unwrap_or(0) > 0,
        "pull details must carry the import receipt: {}",
        receipt["details"]
    );

    // The imported DB is usable and contains the pushed project.
    let (code, stdout, stderr) = run(&["--db", db2.to_str().unwrap(), "list"]);
    assert_eq!(code, 0, "list on imported DB failed: {stderr}");
    assert!(
        stdout.contains("hubproj"),
        "imported DB must contain the project, got: {stdout}"
    );

    shutdown.store(true, Ordering::SeqCst);
    server.join().expect("registry thread");
}

/// A tampered payload must fail the BLAKE3/size gate instead of being
/// imported.
#[test]
fn hub_pull_rejects_tampered_artifact() {
    let (base, registry, shutdown, server) = spawn_registry();
    let dir = tempfile::TempDir::new().expect("tempdir");
    let (_repo, db) = seed_project(dir.path(), "hubproj");
    let db = db.to_str().unwrap().to_string();

    let (code, _stdout, stderr) = run(&[
        "hub",
        "--action",
        "push",
        "--url",
        &base,
        "--project",
        "hubproj",
        "--db",
        &db,
    ]);
    assert_eq!(code, 0, "push failed: {stderr}");

    // Flip one byte in the middle of the uploaded payload; the manifest digest
    // and size no longer match.
    {
        let mut stored = registry.stored.lock().unwrap();
        let bytes = stored.as_mut().expect("pushed payload");
        let idx = bytes.len() / 2;
        bytes[idx] ^= 0xff;
    }

    let db2 = dir.path().join("tampered.lbug");
    let (code, stdout, stderr) = run(&[
        "hub",
        "--action",
        "pull",
        "--url",
        &base,
        "--name",
        "hubproj",
        "--db",
        db2.to_str().unwrap(),
    ]);
    assert_eq!(
        code, 2,
        "tampered artifact must fail with exit 2; stdout={stdout} stderr={stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("integrity")
            || combined.contains("digest")
            || combined.contains("artifact"),
        "error must name the artifact/integrity failure: {combined}"
    );

    shutdown.store(true, Ordering::SeqCst);
    server.join().expect("registry thread");
}
