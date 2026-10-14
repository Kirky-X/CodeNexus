// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! 内嵌 graph-viewer 服务（`webui` feature）。
//!
//! 原独立 graph server 的 HTTP 后端已并入主二进制：
//! 仅绑定 `127.0.0.1`；`/api` 路由经 `x-graph-token` 鉴权并校验 Host 回环；
//! 静态资源来自编译期嵌入的 `graph-viewer/dist`（缺失时呈回退页）。
//!
//! 入口：[`serve`]（CLI 子命令 `codenexus webui` 调用）。

use thiserror::Error;

mod assets;
#[cfg(feature = "cli")]
mod cli;
mod graph;
mod server;

pub use server::serve;

/// `/api/projects` 响应项。
#[derive(Clone, Debug, serde::Serialize)]
pub struct ProjectInfo {
    /// 项目名（`.lbug` 文件主干名）。
    pub name: String,
    /// 发现该库的搜索路径。
    pub root_path: String,
    /// 库文件路径。
    pub db_path: String,
    /// 节点计数。
    pub node_count: u64,
    /// 边计数。
    pub edge_count: u64,
}

/// 图节点（与旧 graph-server 的 DTO 字段逐字一致）。
#[derive(Clone, serde::Serialize)]
pub struct GraphNode {
    /// 前端合成 id。
    pub id: String,
    /// 节点类型标签。
    pub label: String,
    /// 符号名。
    pub name: String,
    /// 所在文件路径。
    pub file_path: Option<String>,
    /// 所属项目。
    pub project: String,
    /// 限定名。
    pub qualified_name: Option<String>,
    /// 起始行。
    pub start_line: Option<u32>,
    /// 结束行。
    pub end_line: Option<u32>,
    /// 球面坐标 x。
    pub x: f64,
    /// 球面坐标 y。
    pub y: f64,
    /// 球面坐标 z。
    pub z: f64,
}

/// 图边。
#[derive(Clone, serde::Serialize)]
pub struct GraphEdge {
    /// 合成 id。
    pub id: String,
    /// 源节点 id。
    pub source: String,
    /// 目标节点 id。
    pub target: String,
    /// 关系类型。
    pub edge_type: String,
    /// 置信度。
    pub confidence: f32,
    /// 起始行。
    pub start_line: Option<u32>,
    /// 所属项目。
    pub project: String,
}

/// `/api/graph` 响应。
#[derive(serde::Serialize)]
pub struct GraphData {
    /// 节点集合。
    pub nodes: Vec<GraphNode>,
    /// 边集合。
    pub edges: Vec<GraphEdge>,
    /// 节点总数（含 stub）。
    pub total_nodes: u64,
    /// 边总数。
    pub total_edges: u64,
}

/// `/api/schema` 响应。
#[derive(serde::Serialize)]
pub struct SchemaInfo {
    /// 各标签节点计数（降序）。
    pub node_labels: Vec<LabelCount>,
    /// 各关系类型计数。
    pub edge_types: Vec<TypeCount>,
    /// 节点总数。
    pub total_nodes: u64,
    /// 边总数。
    pub total_edges: u64,
}

/// 标签计数项。
#[derive(serde::Serialize)]
pub struct LabelCount {
    /// 标签名。
    pub label: String,
    /// 计数。
    pub count: u64,
}

/// 关系类型计数项。
#[derive(serde::Serialize)]
pub struct TypeCount {
    /// 类型名。
    pub r#type: String,
    /// 计数。
    pub count: u64,
}

/// `/api/trace` 响应。
#[derive(serde::Serialize)]
pub struct TraceResult {
    /// 起点节点。
    pub origin: GraphNode,
    /// 追踪路径集合。
    pub paths: Vec<TracePath>,
    /// 方向（downstream/upstream/both）。
    pub direction: String,
}

/// 单条追踪路径。
#[derive(serde::Serialize)]
pub struct TracePath {
    /// 路径节点。
    pub nodes: Vec<GraphNode>,
    /// 路径边。
    pub edges: Vec<GraphEdge>,
}

/// 错误响应体（线契约 `{"error": ...}`；与 sdforge 的 `ApiError` 区分命名）。
#[derive(serde::Serialize)]
pub struct ErrorBody {
    /// 本地化错误文案。
    pub error: String,
}

/// viewer 服务配置。
#[derive(Clone, Debug)]
pub struct ViewerConfig {
    /// 监听端口（仅绑定 `127.0.0.1`）。
    pub port: u16,
}

/// viewer 服务错误。
#[derive(Debug, Error)]
pub enum ViewerError {
    /// 绑定监听地址失败（如端口被占用）。
    #[error("failed to bind 127.0.0.1:{port}: {source}")]
    Bind {
        /// 目标端口。
        port: u16,
        /// 底层 IO 错误。
        #[source]
        source: std::io::Error,
    },
    /// 服务运行期 IO 错误。
    #[error("viewer server io error: {0}")]
    Io(#[from] std::io::Error),
}
