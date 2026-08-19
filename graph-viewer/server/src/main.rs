// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! CodeNexus 图数据可视化后端服务
//!
//! 读取 LadybugDB (.lbug) 数据库文件，通过 REST API 提供图数据查询，
//! 支持节点/边查询、筛选过滤、函数调用追踪和变量使用追踪。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

mod graph_query;

/// 服务配置
#[derive(Clone)]
struct AppState {
    /// 已索引项目的 .lbug 文件路径映射
    projects: Arc<RwLock<Vec<ProjectEntry>>>,
    /// 默认搜索路径（CodeNexus 的 .codenexus/ 目录）
    search_paths: Vec<PathBuf>,
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
    lbug_path: Option<String>,
    max_nodes: Option<u64>,
    file_path: Option<String>,
}

#[derive(Deserialize)]
struct TraceQuery {
    project: Option<String>,
    lbug_path: Option<String>,
    node_id: String,
    mode: String,
    direction: Option<String>,
    max_depth: Option<u32>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("graph_server=info,axum=info")
        .init();

    let search_paths = vec![
        PathBuf::from(".codenexus"),
        PathBuf::from("."),
        PathBuf::from("/home/kirky/projects/CodeNexus/.codenexus"),
        PathBuf::from("/home/kirky/projects/CodeNexus"),
    ];

    let state = AppState {
        projects: Arc::new(RwLock::new(Vec::new())),
        search_paths,
    };

    /* 扫描可用项目 */
    scan_projects(&state).await;

    let app = Router::new()
        .route("/api/projects", get(list_projects))
        .route("/api/graph", get(get_graph))
        .route("/api/schema", get(get_schema))
        .route("/api/trace", get(get_trace))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:9800")
        .await
        .expect("Failed to bind to 127.0.0.1:9800");

    tracing::info!("图数据服务已启动: http://127.0.0.1:9800");

    axum::serve(listener, app).await.expect("Server error");
}

async fn scan_projects(state: &AppState) {
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
                        tracing::info!("发现项目: {} -> {}", name, projects.last().unwrap().db_path.display());
                    }
                }
            }
        }
    }
}

async fn list_projects(State(state): State<AppState>) -> impl IntoResponse {
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

/// 解析 db_path：优先 lbug_path，其次按 project 名查找
fn resolve_db_path<'a>(
    projects: &'a [ProjectEntry],
    project_name: Option<&str>,
    lbug_path: Option<&str>,
) -> Result<(PathBuf, String), String> {
    if let Some(path) = lbug_path {
        let p = PathBuf::from(path);
        if !p.exists() {
            return Err(format!("文件不存在: {}", path));
        }
        let name = p.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        return Ok((p, name));
    }
    let name = project_name.ok_or("需要提供 project 或 lbug_path 参数")?;
    let project = projects.iter().find(|p| p.name == name)
        .ok_or_else(|| format!("项目 '{}' 未找到", name))?;
    Ok((project.db_path.clone(), name.to_string()))
}

async fn get_graph(
    State(state): State<AppState>,
    Query(query): Query<GraphQuery>,
) -> impl IntoResponse {
    let projects = state.projects.read().await;
    let (db_path, project_name) = match resolve_db_path(
        &projects,
        query.project.as_deref(),
        query.lbug_path.as_deref(),
    ) {
        Ok(v) => v,
        Err(e) => return (StatusCode::NOT_FOUND, Json(ApiError { error: e })).into_response(),
    };

    match graph_query::query_graph(&db_path, &project_name, query.max_nodes, query.file_path.as_deref()) {
        Ok(data) => (StatusCode::OK, Json(serde_json::to_value(data).unwrap())).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e.to_string() }),
        ).into_response(),
    }
}

async fn get_schema(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let project_name = params.get("project").cloned();
    let lbug_path = params.get("lbug_path").cloned();
    let projects = state.projects.read().await;
    let (db_path, _) = match resolve_db_path(
        &projects,
        project_name.as_deref(),
        lbug_path.as_deref(),
    ) {
        Ok(v) => v,
        Err(e) => return (StatusCode::NOT_FOUND, Json(ApiError { error: e })).into_response(),
    };

    match graph_query::query_schema(&db_path) {
        Ok(schema) => (StatusCode::OK, Json(serde_json::to_value(schema).unwrap())).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e.to_string() }),
        ).into_response(),
    }
}

async fn get_trace(
    State(state): State<AppState>,
    Query(query): Query<TraceQuery>,
) -> impl IntoResponse {
    let projects = state.projects.read().await;
    let (db_path, project_name) = match resolve_db_path(
        &projects,
        query.project.as_deref(),
        query.lbug_path.as_deref(),
    ) {
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
            Json(ApiError { error: e.to_string() }),
        ).into_response(),
    }
}
