// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! viewer HTTP 服务：路由、鉴权中间件、静态资源与查询 handlers。
//!
//! # 安全模型（沿用旧独立 graph server）
//!
//! - 数据库只从启动时扫描的白名单目录（`.codenexus` + cwd）解析，API 不接受
//!   任意 `lbug_path` 参数（否则本机任意进程/浏览器可经此读取文件系统上
//!   任意 `.lbug` 库中的源码内容）。
//! - 所有 `/api` 路由要求启动时生成、打印到 stdout 的随机 token
//!   （`x-graph-token` 头），防止本机其他进程或 DNS rebinding 页面被动
//!   拖走索引中的源码；静态资源（前端构建产物）为公开面、不受 token 限制。
//! - `/api` 校验 Host 头只允许本机回环名，拒绝 rebinding 域名。

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{Query, Request, State},
    http::{HeaderMap, StatusCode, Uri},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::i18n::{t, tr};

use super::graph;
use super::{ErrorBody, ProjectInfo, ViewerConfig, ViewerError};

/// 服务状态：已发现项目、白名单搜索路径与鉴权 token。
#[derive(Clone)]
pub(super) struct ViewerState {
    /// 已索引项目的 .lbug 文件路径映射。
    projects: Arc<RwLock<Vec<ProjectEntry>>>,
    /// 默认搜索路径（CodeNexus 的 .codenexus/ 目录）——数据库白名单来源。
    search_paths: Vec<PathBuf>,
    /// 启动时生成的随机 token（`x-graph-token` 头校验用）。
    auth_token: Arc<String>,
}

#[derive(Clone, Debug)]
struct ProjectEntry {
    name: String,
    root_path: String,
    db_path: PathBuf,
}

/// `/api/graph` 查询参数。
#[derive(Deserialize)]
struct GraphQuery {
    project: Option<String>,
    max_nodes: Option<u64>,
    file_path: Option<String>,
}

/// `/api/trace` 查询参数。
#[derive(Deserialize)]
struct TraceQuery {
    project: Option<String>,
    node_id: String,
    mode: String,
    direction: Option<String>,
    max_depth: Option<u32>,
}

/// 组装完整路由：`/api`（带鉴权） + 静态资源 fallback。
pub(super) fn router(state: ViewerState) -> Router {
    let api = Router::new()
        .route("/api/projects", get(list_projects))
        .route("/api/graph", get(get_graph))
        .route("/api/schema", get(get_schema))
        .route("/api/trace", get(get_trace))
        .layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state);
    Router::new().merge(api).fallback(serve_asset)
}

/// 启动内嵌 viewer 服务（阻塞直到进程退出）。
///
/// # Errors
///
/// 返回 [`ViewerError::Bind`] 当 `127.0.0.1:<port>` 无法绑定；
/// 返回 [`ViewerError::Io`] 当服务运行期出现 IO 错误。
pub async fn serve(config: ViewerConfig) -> Result<(), ViewerError> {
    let search_paths = vec![PathBuf::from(".codenexus"), PathBuf::from(".")];
    let auth_token = Arc::new(generate_auth_token());
    let state = ViewerState {
        projects: Arc::new(RwLock::new(Vec::new())),
        search_paths,
        auth_token: Arc::clone(&auth_token),
    };

    /* 扫描可用项目 */
    scan_projects(&state).await;

    let addr = SocketAddr::from(([127, 0, 0, 1], config.port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|source| ViewerError::Bind {
            port: config.port,
            source,
        })?;

    /* bind 成功后打印 token：浏览器/脚本经 `x-graph-token` 头携带 */
    println!("graph-server auth token: {auth_token}");
    tracing::info!(
        "{}",
        t("graph-server-started", &[("port", config.port.to_string())])
    );

    axum::serve(listener, router(state)).await?;
    Ok(())
}

/// 从操作系统熵源生成 256-bit 随机 token 的十六进制串。
///
/// Unix 优先读 `/dev/urandom`；不可用时退化为时间戳 + pid 混合熵
/// （弱于真随机，但仍远好于固定 token——本服务只绑定回环地址，属纵深防御）。
fn generate_auth_token() -> String {
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
    #[cfg(unix)]
    {
        use std::io::Read;
        let mut buf = [0u8; 32];
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            if f.read_exact(&mut buf).is_ok() {
                return hex(&buf);
            }
        }
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    hex(&((nanos ^ (pid << 96)).to_le_bytes()))
}

/// 解析 Host 头中的主机名（剥端口；容忍 `[::1]:9800` 形式）。
fn host_name(host_header: &str) -> &str {
    if let Some(rest) = host_header.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(host_header);
    }
    host_header
        .rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or(host_header)
}

/// 鉴权 + Host 校验中间件（仅作用于 `/api` 路由）。
async fn require_auth(
    State(state): State<ViewerState>,
    headers: HeaderMap,
    req: Request,
    next: Next,
) -> Response {
    // Host 校验：拒绝 DNS rebinding（恶意页面把自有域名解析到 127.0.0.1）。
    let host_ok = headers
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(host_name)
        .is_some_and(|h| matches!(h, "127.0.0.1" | "localhost" | "::1"));
    if !host_ok {
        return (
            StatusCode::FORBIDDEN,
            Json(ErrorBody {
                error: tr("graph-host-forbidden"),
            }),
        )
            .into_response();
    }
    // Token 校验。
    let token_ok = headers
        .get("x-graph-token")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == state.auth_token.as_str());
    if !token_ok {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ErrorBody {
                error: tr("graph-token-missing"),
            }),
        )
            .into_response();
    }
    next.run(req).await
}

/// 静态资源：命中即回；`assets/` 未命中 → 404；其余未知路径回落 index.html
/// （前端 SPA 语义）；`dist` 缺失（未嵌入）时呈回退页。
async fn serve_asset(uri: Uri) -> Response {
    let raw = uri.path().trim_start_matches('/');
    let rel = if raw.is_empty() { "index.html" } else { raw };
    /* 防目录穿越：拒绝任何 `..` 段 */
    if rel.split('/').any(|seg| seg == "..") {
        return (StatusCode::BAD_REQUEST, "bad path").into_response();
    }
    if let Some((bytes, content_type)) = super::assets::get_asset(rel) {
        return ([(axum::http::header::CONTENT_TYPE, content_type)], bytes).into_response();
    }
    if rel.starts_with("assets/") {
        return (StatusCode::NOT_FOUND, "asset not found").into_response();
    }
    if let Some((bytes, content_type)) = super::assets::get_asset("index.html") {
        return ([(axum::http::header::CONTENT_TYPE, content_type)], bytes).into_response();
    }
    Html(super::assets::fallback_page()).into_response()
}

/// 扫描白名单目录中的 `.lbug` 库（canonicalize 去重）。
async fn scan_projects(state: &ViewerState) {
    let mut projects = state.projects.write().await;
    projects.clear();
    let mut seen = HashSet::new();

    for search_path in &state.search_paths {
        if !search_path.exists() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(search_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "lbug") {
                    /* 通过 canonicalize 去重 */
                    let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
                    if !seen.insert(canonical.clone()) {
                        continue;
                    }
                    let name = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if !name.is_empty() {
                        projects.push(ProjectEntry {
                            name: name.clone(),
                            root_path: search_path.to_string_lossy().to_string(),
                            db_path: canonical,
                        });
                        tracing::info!(
                            "{}",
                            t(
                                "graph-project-discovered",
                                &[
                                    ("name", name.clone()),
                                    (
                                        "path",
                                        projects.last().unwrap().db_path.display().to_string()
                                    ),
                                ]
                            )
                        );
                    }
                }
            }
        }
    }
}

async fn list_projects(State(state): State<ViewerState>) -> impl IntoResponse {
    let projects = state.projects.read().await;
    let infos: Vec<ProjectInfo> = projects
        .iter()
        .map(|p| {
            let (nc, ec) = graph::quick_count(&p.db_path).unwrap_or((0, 0));
            ProjectInfo {
                name: p.name.clone(),
                root_path: p.root_path.clone(),
                db_path: p.db_path.to_string_lossy().to_string(),
                node_count: nc,
                edge_count: ec,
            }
        })
        .collect();
    Json(infos)
}

/// 解析 db_path：按 project 名在扫描白名单中查找。
///
/// 曾支持 `lbug_path` 参数直接指定任意路径——那是一个任意 DB 读取原语
/// （`Function` 表含源码正文），且叠加 DNS rebinding 可被浏览器被动利用；
/// 前端实际走浏览器内 WASM 查询，不消费该参数，故保持移除。
fn resolve_db_path(
    projects: &[ProjectEntry],
    project_name: Option<&str>,
) -> Result<(PathBuf, String), String> {
    let name = project_name.ok_or_else(|| tr("project-param-required"))?;
    let project = projects
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| t("project-not-found", &[("name", name.to_string())]))?;
    Ok((project.db_path.clone(), name.to_string()))
}

async fn get_graph(
    State(state): State<ViewerState>,
    Query(query): Query<GraphQuery>,
) -> impl IntoResponse {
    let projects = state.projects.read().await;
    let (db_path, project_name) = match resolve_db_path(&projects, query.project.as_deref()) {
        Ok(v) => v,
        Err(e) => return (StatusCode::NOT_FOUND, Json(ErrorBody { error: e })).into_response(),
    };

    match graph::query_graph(
        &db_path,
        &project_name,
        query.max_nodes,
        query.file_path.as_deref(),
    ) {
        Ok(data) => (StatusCode::OK, Json(data)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn get_schema(
    State(state): State<ViewerState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let project_name = params.get("project").cloned();
    let projects = state.projects.read().await;
    let (db_path, _) = match resolve_db_path(&projects, project_name.as_deref()) {
        Ok(v) => v,
        Err(e) => return (StatusCode::NOT_FOUND, Json(ErrorBody { error: e })).into_response(),
    };

    match graph::query_schema(&db_path) {
        Ok(schema) => (StatusCode::OK, Json(schema)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn get_trace(
    State(state): State<ViewerState>,
    Query(query): Query<TraceQuery>,
) -> impl IntoResponse {
    let projects = state.projects.read().await;
    let (db_path, project_name) = match resolve_db_path(&projects, query.project.as_deref()) {
        Ok(v) => v,
        Err(e) => return (StatusCode::NOT_FOUND, Json(ErrorBody { error: e })).into_response(),
    };

    let direction = query.direction.as_deref().unwrap_or("both");
    let max_depth = query.max_depth.unwrap_or(10);

    match graph::query_trace(
        &db_path,
        &project_name,
        &query.node_id,
        &query.mode,
        direction,
        max_depth,
    ) {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Language, Node, NodeLabel};
    use crate::storage::Repository;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt;

    const TOKEN: &str = "test-token";

    /// 建临时库（1 个 Function）并返回（guard, state）。
    fn test_state() -> (tempfile::TempDir, ViewerState) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("demo.lbug");
        let repo = Repository::open(&db_path).expect("open");
        let node = Node::builder(NodeLabel::Function, "alpha", "demo.alpha")
            .id("f0")
            .project("demo")
            .file_path("/src/main.rs")
            .start_line(1)
            .end_line(10)
            .language(Language::Rust)
            .signature("fn main()")
            .build();
        repo.save_nodes_stream(&[node], NodeLabel::Function)
            .expect("save_nodes");
        drop(repo);
        let state = ViewerState {
            projects: Arc::new(RwLock::new(vec![ProjectEntry {
                name: "demo".to_string(),
                root_path: dir.path().to_string_lossy().to_string(),
                db_path,
            }])),
            search_paths: vec![],
            auth_token: Arc::new(TOKEN.to_string()),
        };
        (dir, state)
    }

    fn request(path: &str, host: &str, token: Option<&str>) -> HttpRequest<Body> {
        let mut builder = HttpRequest::builder().uri(path).header("host", host);
        if let Some(t) = token {
            builder = builder.header("x-graph-token", t);
        }
        builder.body(Body::empty()).expect("request")
    }

    #[tokio::test]
    async fn static_root_serves_html_without_token() {
        let (_dir, state) = test_state();
        let res = router(state)
            .oneshot(request("/", "127.0.0.1", None))
            .await
            .expect("oneshot");
        assert_eq!(res.status(), StatusCode::OK);
        let ct = res
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(ct.starts_with("text/html"), "content-type: {ct}");
    }

    #[tokio::test]
    async fn missing_asset_under_assets_prefix_is_404() {
        let (_dir, state) = test_state();
        let res = router(state)
            .oneshot(request("/assets/nope.js", "127.0.0.1", None))
            .await
            .expect("oneshot");
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn path_traversal_is_rejected() {
        let (_dir, state) = test_state();
        let res = router(state)
            .oneshot(request("/../Cargo.toml", "127.0.0.1", None))
            .await
            .expect("oneshot");
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn lbug_fixture_path_never_serves_fixture_bytes() {
        let (_dir, state) = test_state();
        let res = router(state)
            .oneshot(request("/test_e2e.lbug", "127.0.0.1", None))
            .await
            .expect("oneshot");
        // fixture 未嵌入：请求该路径只会回落 index.html/回退页（HTML），不可能返回其字节
        assert_eq!(res.status(), StatusCode::OK);
        let ct = res
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(ct.starts_with("text/html"), "content-type: {ct}");
    }

    #[tokio::test]
    async fn api_requires_token() {
        let (_dir, state) = test_state();
        let res = router(state)
            .oneshot(request("/api/projects", "127.0.0.1", None))
            .await
            .expect("oneshot");
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_rejects_wrong_token() {
        let (_dir, state) = test_state();
        let res = router(state)
            .oneshot(request("/api/projects", "127.0.0.1", Some("nope")))
            .await
            .expect("oneshot");
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_rejects_non_loopback_host() {
        let (_dir, state) = test_state();
        let res = router(state)
            .oneshot(request("/api/projects", "evil.example.com", Some(TOKEN)))
            .await
            .expect("oneshot");
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn api_projects_lists_discovered_project() {
        let (_dir, state) = test_state();
        let res = router(state)
            .oneshot(request("/api/projects", "127.0.0.1", Some(TOKEN)))
            .await
            .expect("oneshot");
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json[0]["name"], "demo");
        // 1 个 Function + SchemaMeta 单行表（与旧 server 的同一条 count 查询一致）
        assert_eq!(json[0]["node_count"], 2);
        assert_eq!(json[0]["edge_count"], 0);
    }

    #[tokio::test]
    async fn api_schema_and_graph_and_trace_respond() {
        let (_dir, state) = test_state();
        let app = router(state);

        let schema = app
            .clone()
            .oneshot(request(
                "/api/schema?project=demo",
                "127.0.0.1",
                Some(TOKEN),
            ))
            .await
            .expect("oneshot");
        assert_eq!(schema.status(), StatusCode::OK);
        let body = axum::body::to_bytes(schema.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json["total_nodes"], 1);
        assert_eq!(json["node_labels"][0]["label"], "Function");

        let graph = app
            .clone()
            .oneshot(request("/api/graph?project=demo", "127.0.0.1", Some(TOKEN)))
            .await
            .expect("oneshot");
        assert_eq!(graph.status(), StatusCode::OK);

        let trace = app
            .oneshot(request(
                "/api/trace?project=demo&node_id=alpha&mode=call&direction=downstream&max_depth=3",
                "127.0.0.1",
                Some(TOKEN),
            ))
            .await
            .expect("oneshot");
        assert_eq!(trace.status(), StatusCode::OK);
        let body = axum::body::to_bytes(trace.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json["origin"]["name"], "alpha");
        assert_eq!(json["direction"], "downstream");
    }

    #[tokio::test]
    async fn unknown_project_is_404() {
        let (_dir, state) = test_state();
        let res = router(state)
            .oneshot(request(
                "/api/schema?project=absent",
                "127.0.0.1",
                Some(TOKEN),
            ))
            .await
            .expect("oneshot");
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn serve_reports_bind_error_when_port_taken() {
        // 端口占用：serve 必须返回 ViewerError::Bind（不 panic；R-005 验收）。
        let holder = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("hold port");
        let port = holder.local_addr().expect("addr").port();
        let err = serve(ViewerConfig { port })
            .await
            .expect_err("port busy must fail");
        assert!(matches!(err, ViewerError::Bind { port: p, .. } if p == port));
        drop(holder);
    }

    #[test]
    fn host_name_strips_port_and_brackets() {
        assert_eq!(host_name("127.0.0.1:9800"), "127.0.0.1");
        assert_eq!(host_name("localhost"), "localhost");
        assert_eq!(host_name("[::1]:9800"), "::1");
        assert_eq!(host_name("[::1]"), "::1");
    }
}
