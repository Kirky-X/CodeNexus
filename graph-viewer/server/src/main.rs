// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! CodeNexus 图数据可视化后端服务
//!
//! 读取 LadybugDB (.lbug) 数据库文件，通过 REST API 提供图数据查询，
//! 支持节点/边查询、筛选过滤、函数调用追踪和变量使用追踪。
//!
//! # 安全模型
//!
//! - 数据库只从启动时扫描的白名单目录（`search_paths`）解析，API 不接受
//!   任意 `lbug_path` 参数（否则本机任意进程/浏览器可经此读取文件系统上
//!   任意 `.lbug` 库中的源码内容）。
//! - 所有 `/api` 路由要求启动时生成、打印到 stdout 的随机 token
//!   （`x-graph-token` 头），防止本机其他进程或 DNS rebinding 页面被动
//!   拖走索引中的源码。
//! - 校验 Host 头只允许本机回环名，拒绝 rebinding 域名。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

mod graph_query;
mod i18n;

/// 服务配置
#[derive(Clone)]
struct CodeNexusState {
    /// 已索引项目的 .lbug 文件路径映射
    projects: Arc<RwLock<Vec<ProjectEntry>>>,
    /// 默认搜索路径（CodeNexus 的 .codenexus/ 目录）——数据库白名单来源
    search_paths: Vec<PathBuf>,
    /// 启动时生成的随机 token（`x-graph-token` 头校验用）
    auth_token: Arc<String>,
}

#[derive(Clone, Debug)]
struct ProjectEntry {
    name: String,
    root_path: String,
    db_path: PathBuf,
}

/// API 响应类型
#[derive(Serialize)]
struct ProjectInfo {
    name: String,
    root_path: String,
    db_path: String,
    node_count: u64,
    edge_count: u64,
}

#[derive(Serialize, Clone)]
struct GraphNode {
    id: String,
    label: String,
    name: String,
    file_path: Option<String>,
    project: String,
    qualified_name: Option<String>,
    start_line: Option<u32>,
    end_line: Option<u32>,
    x: f64,
    y: f64,
    z: f64,
}

#[derive(Serialize, Clone)]
struct GraphEdge {
    id: String,
    source: String,
    target: String,
    edge_type: String,
    confidence: f32,
    start_line: Option<u32>,
    project: String,
}

#[derive(Serialize)]
struct GraphData {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
    total_nodes: u64,
    total_edges: u64,
}

#[derive(Serialize)]
struct SchemaInfo {
    node_labels: Vec<LabelCount>,
    edge_types: Vec<TypeCount>,
    total_nodes: u64,
    total_edges: u64,
}

#[derive(Serialize)]
struct LabelCount {
    label: String,
    count: u64,
}

#[derive(Serialize)]
struct TypeCount {
    r#type: String,
    count: u64,
}

#[derive(Serialize)]
struct TraceResult {
    origin: GraphNode,
    paths: Vec<TracePath>,
    direction: String,
}

#[derive(Serialize)]
struct TracePath {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
}

#[derive(Serialize)]
struct ApiError {
    error: String,
}

/// 查询参数
#[derive(Deserialize)]
struct GraphQuery {
    project: Option<String>,
    max_nodes: Option<u64>,
    file_path: Option<String>,
}

#[derive(Deserialize)]
struct TraceQuery {
    project: Option<String>,
    node_id: String,
    mode: String,
    direction: Option<String>,
    max_depth: Option<u32>,
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
        if let Ok(mut f) = std::fs::File::open("/dev/urandom")
            && f.read_exact(&mut buf).is_ok()
        {
            return hex(&buf);
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

/// 鉴权 + Host 校验中间件。
async fn require_auth(
    State(state): State<CodeNexusState>,
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
            Json(ApiError {
                error: i18n::tr("graph-host-forbidden"),
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
            Json(ApiError {
                error: i18n::tr("graph-token-missing"),
            }),
        )
            .into_response();
    }
    next.run(req).await
}

#[tokio::main]
async fn main() {
    // Message i18n: detect once up front (lazy fallback inside i18n::t
    // guarantees localized output even without this call).
    i18n::init();

    // 生产日志由 inklog 接管（全局 info 级；原 tracing-subscriber 的
    // per-target filter 不再支持，对单用途 dev server 影响可忽略）
    let logger = inklog::LoggerManager::builder()
        .level("info")
        .format("{timestamp} [{level}] {target} - {message}")
        .console(true)
        .console_colored(true)
        .build()
        .await
        .expect("init inklog");
    std::mem::forget(logger);

    let search_paths = vec![PathBuf::from(".codenexus"), PathBuf::from(".")];

    let auth_token = Arc::new(generate_auth_token());
    let state = CodeNexusState {
        projects: Arc::new(RwLock::new(Vec::new())),
        search_paths,
        auth_token: Arc::clone(&auth_token),
    };

    /* 扫描可用项目 */
    scan_projects(&state).await;

    let app = Router::new()
        .route("/api/projects", get(list_projects))
        .route("/api/graph", get(get_graph))
        .route("/api/schema", get(get_schema))
        .route("/api/trace", get(get_trace))
        .layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:9800")
        .await
        .expect("Failed to bind to 127.0.0.1:9800");

    println!("graph-server auth token: {auth_token}");
    tracing::info!("{}", i18n::tr("graph-server-started"));

    axum::serve(listener, app).await.expect("Server error");
}

async fn scan_projects(state: &CodeNexusState) {
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
                            i18n::t(
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

async fn list_projects(State(state): State<CodeNexusState>) -> impl IntoResponse {
    let projects = state.projects.read().await;
    let infos: Vec<ProjectInfo> = projects
        .iter()
        .map(|p| {
            let (nc, ec) = graph_query::quick_count(&p.db_path).unwrap_or((0, 0));
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
/// 前端实际走浏览器内 WASM 查询，不消费该参数，故已移除。
fn resolve_db_path(
    projects: &[ProjectEntry],
    project_name: Option<&str>,
) -> Result<(PathBuf, String), String> {
    let name = project_name.ok_or_else(|| i18n::tr("project-param-required"))?;
    let project = projects
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| i18n::t("project-not-found", &[("name", name.to_string())]))?;
    Ok((project.db_path.clone(), name.to_string()))
}

async fn get_graph(
    State(state): State<CodeNexusState>,
    Query(query): Query<GraphQuery>,
) -> impl IntoResponse {
    let projects = state.projects.read().await;
    let (db_path, project_name) = match resolve_db_path(&projects, query.project.as_deref()) {
        Ok(v) => v,
        Err(e) => return (StatusCode::NOT_FOUND, Json(ApiError { error: e })).into_response(),
    };

    match graph_query::query_graph(
        &db_path,
        &project_name,
        query.max_nodes,
        query.file_path.as_deref(),
    ) {
        Ok(data) => (StatusCode::OK, Json(serde_json::to_value(data).unwrap())).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn get_schema(
    State(state): State<CodeNexusState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let project_name = params.get("project").cloned();
    let projects = state.projects.read().await;
    let (db_path, _) = match resolve_db_path(&projects, project_name.as_deref()) {
        Ok(v) => v,
        Err(e) => return (StatusCode::NOT_FOUND, Json(ApiError { error: e })).into_response(),
    };

    match graph_query::query_schema(&db_path) {
        Ok(schema) => (StatusCode::OK, Json(serde_json::to_value(schema).unwrap())).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn get_trace(
    State(state): State<CodeNexusState>,
    Query(query): Query<TraceQuery>,
) -> impl IntoResponse {
    let projects = state.projects.read().await;
    let (db_path, project_name) = match resolve_db_path(&projects, query.project.as_deref()) {
        Ok(v) => v,
        Err(e) => return (StatusCode::NOT_FOUND, Json(ApiError { error: e })).into_response(),
    };

    let direction = query.direction.as_deref().unwrap_or("both");
    let max_depth = query.max_depth.unwrap_or(10);

    match graph_query::query_trace(
        &db_path,
        &project_name,
        &query.node_id,
        &query.mode,
        direction,
        max_depth,
    ) {
        Ok(result) => (StatusCode::OK, Json(serde_json::to_value(result).unwrap())).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}
