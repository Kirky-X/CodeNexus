# CodeNexus Hub Protocol v1

> Status: draft v1 · Introduced by change `feature-expansion-wave` (2026-09)
> Client: `codenexus hub` · Reference env token: `CODENEXUS_HUB_TOKEN`

## 概述

Hub 协议定义 CodeNexus CLI 与制品注册服务器之间推送/拉取/枚举 `.cnxp` 索引制品的最小 HTTP 契约。**服务器不在本仓库范围内**——任何实现以下端点的服务都可作为注册中心。

数据仅在显式 `hub push`（或显式配置的 daemon webhook）时离开本机；CLI 不含任何遥测。

## 端点

| 操作 | 方法 | 路径 | 请求体 | 成功响应 |
|------|------|------|--------|----------|
| push | `POST` | `{base}/artifacts` | `application/octet-stream`（`.cnxp` 二进制） | `200/201` + JSON `{"id": "...", "url": "..."}` |
| pull | `GET` | `{base}/artifacts/{name}/latest` | — | `200` + `.cnxp` 二进制 |
| list | `GET` | `{base}/artifacts?project={p}` | — | `200` + JSON 数组 |

- `{base}`：注册服务器基址（`http(s)://host[:port]`，无尾斜杠）。
- `{name}`：制品名（URL 路径段，非保留字符按 RFC 3986 百分号编码）。
- `{p}`：项目名（URL query 值，同样编码）。

## 请求头

| 头 | 方向 | 说明 |
|----|------|------|
| `Authorization: Bearer <token>` | 客户端 → 服务端 | 可选；token 取 `--token` 参数或环境变量 `CODENEXUS_HUB_TOKEN` |
| `X-Codenexus-Project: <project>` | push | 推送来源项目名（服务端可用于归属/配额） |
| `Content-Type: application/octet-stream` | push | 制品为二进制 `.cnxp` 容器 |

## 错误约定

非 2xx 视为失败。客户端将状态码与响应体前 200 字符并入错误消息（exit 2）。建议服务端错误体为 JSON：

```json
{ "error": "conflict", "message": "artifact already exists with different digest" }
```

推荐状态码：`400` 请求非法 · `401/403` 鉴权失败 · `404` 制品不存在 · `409` 冲突 · `413` 超限 · `500` 服务端错误。

## `.cnxp` 完整性要求

- 容器格式：`[magic "CNXP" 4B][manifest_len u32 LE][manifest JSON][zstd 压缩 DB]`（`ARTIFACT_FORMAT_VERSION = "1.0"`）。
- 客户端 pull 后走本地 `import` 通路：BLAKE3 摘要校验（manifest `payload_blake3`）+ `original_size` 比对 + 4 GiB 解压上限。
- 服务端应将制品视为不透明二进制；除了大小上限（建议 ≥ 4 GiB）外无需理解内容。

## Token 约定

- 优先级：`--token` 参数 > `CODENEXUS_HUB_TOKEN` 环境变量 > 匿名。
- v1 只有单一 bearer token，无作用域/过期语义（留待 v2）。

## v1 演进边界

明确**不做**：分块上传/断点续传、多制品版本管理（仅 `latest`）、签名与鉴权体系、服务端仓库内实现。字段可能随首个服务端参考实现微调；破坏性变更升 `v2` 前缀路径。
