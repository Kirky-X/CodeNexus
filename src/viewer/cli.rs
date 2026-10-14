// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! `webui` CLI 子命令 — 启动内嵌 graph-viewer Web UI（前端资源已编译进二进制）。

use crate::service::error::{to_api_error, CodeNexusError};
use sdforge::prelude::ApiError;

use super::{serve, ViewerConfig};

/// 默认监听端口（与旧独立 graph server 一致）。
const DEFAULT_WEBUI_PORT: u16 = 9800;

/// CLI wrapper — 一条命令启动内嵌 graph-viewer；`--port` 缺省（或 0）为 9800。
#[cfg(feature = "cli")]
#[sdforge::forge(
    name = "webui",
    version = "0.4.0",
    description = "Start the embedded graph-viewer Web UI (loopback-only; auth token printed on stdout, frontend assets compiled into the binary).",
    cli = true
)]
async fn webui(#[param(kind = "body")] port: Option<u16>) -> Result<(), ApiError> {
    let port = match port {
        Some(p) if p != 0 => p,
        _ => DEFAULT_WEBUI_PORT,
    };
    serve(ViewerConfig { port })
        .await
        .map_err(|e| to_api_error(CodeNexusError::Internal(e.to_string()), "webui_error"))
}
