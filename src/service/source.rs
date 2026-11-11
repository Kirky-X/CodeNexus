// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Read the full source text of any span-bearing symbol from the graph.
//!
//! Core library API（外部可自行封装，不依赖 CLI/MCP）：[`run_read_symbol_source`]
//! 按三种定位键（qualifiedName / 节点 id / filePath+name）从图谱定位符号，
//! 校验索引新鲜度后回源文件切片，返回完整源码片段。
//!
//! 安全面约定：**图谱是唯一路径真源**——读盘路径只取 DB 节点行；
//! 入参中的 filePath 仅作查询键，绝不拼接进读盘路径。

use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use crate::index::hash::{compute_content_hash, MAX_FILE_SIZE};
use crate::kit::{AsyncKit, AsyncReady, StorageModule};
use crate::model::node_label::NodeLabel;
use crate::service::error::CodeNexusError;
use crate::source_io::slice_lines;
use crate::storage::schema::{escape_cypher_string, escape_identifier, node_table_columns};

#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::error::{kit_not_initialized, to_api_error};
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::runtime::kit;
#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::forge;
#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;

/// 默认单次返回的行数上限（超大符号须用行窗口分页）
pub const DEFAULT_MAX_LINES: usize = 500;

/// 定位键：三选一（互斥）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolKey {
    /// qualifiedName 精确匹配（跨重索引稳定的标识）
    QualifiedName(String),
    /// 节点 UUID 主键（单行精确命中；重索引后失效）
    NodeId(String),
    /// 文件路径 + 符号名组合（agent 兜底键）
    FilePathAndName { file_path: String, name: String },
}

impl SymbolKey {
    /// 错误信息里的人类可读回显
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            SymbolKey::QualifiedName(qn) => format!("qualifiedName={qn}"),
            SymbolKey::NodeId(id) => format!("id={id}"),
            SymbolKey::FilePathAndName { file_path, name } => {
                format!("filePath={file_path} name={name}")
            }
        }
    }
}

/// 行窗口：绝对文件行号闭区间；`end` 省略 = 到符号 span 尾
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineWindow {
    pub start: u32,
    pub end: Option<u32>,
}

/// 读取选项：窗口与输出上限
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceOptions {
    pub window: Option<LineWindow>,
    pub max_lines: NonZeroUsize,
}

impl Default for SourceOptions {
    fn default() -> Self {
        Self {
            window: None,
            max_lines: NonZeroUsize::new(DEFAULT_MAX_LINES)
                .expect("DEFAULT_MAX_LINES is a nonzero constant"),
        }
    }
}

/// 符号定位候选（歧义错误里回显，供调用方选键）
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CandidateRef {
    pub node_id: String,
    pub label: String,
    pub qualified_name: String,
}

/// stale 具体原因
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleReason {
    /// 项目根或源文件不在盘上
    FileMissing,
    /// 索引后文件内容已变更（hash 不一致）
    HashMismatch,
    /// 图谱记录的 span 超出文件实际长度
    SpanBeyondFile,
    /// 项目根退化为文件系统根（`/`）：`starts_with` 恒真，穿越防护失效。
    /// 正常索引不可能写出这种根，只会源于被篡改/损坏的索引库。
    InvalidProjectRoot,
}

impl std::fmt::Display for StaleReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StaleReason::FileMissing => write!(f, "source file missing"),
            StaleReason::HashMismatch => write!(f, "file changed since last index"),
            StaleReason::SpanBeyondFile => write!(f, "indexed span exceeds file length"),
            StaleReason::InvalidProjectRoot => write!(
                f,
                "indexed project root is the filesystem root (corrupted index)"
            ),
        }
    }
}

/// 源码读取错误。经 [`From`] 桥接进 [`CodeNexusError`] 获得统一退出码：
/// NotFound→4（找不到），InvalidInput→2（歧义/stale/窗口/超限——调用方可自行纠正），
/// Internal→1（环境故障）。
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("symbol not found: {key}")]
    SymbolNotFound { key: String },

    #[error("ambiguous symbol \"{key}\" matches {} candidates: {}", candidates.len(), candidate_list(candidates))]
    AmbiguousSymbol {
        key: String,
        candidates: Vec<CandidateRef>,
    },

    #[error("stale index for {file_path} ({reason}); run: codenexus index to refresh spans")]
    StaleIndex {
        file_path: String,
        reason: StaleReason,
    },

    #[error("window start {start} is outside symbol span [{span_start}, {span_end}]")]
    WindowOutsideSpan {
        start: u32,
        span_start: u32,
        span_end: u32,
    },

    #[error(
        "symbol spans {total_lines} lines, exceeding max {max_lines}; pass a line window (start/end) to page through"
    )]
    SourceTooLarge {
        total_lines: usize,
        max_lines: usize,
    },

    #[error("source unavailable: {0}")]
    SourceUnavailable(String),
}

/// 歧义候选的紧凑列表：`label:qualifiedName` 分号连接
fn candidate_list(candidates: &[CandidateRef]) -> String {
    candidates
        .iter()
        .map(|c| format!("{}:{}", c.label, c.qualified_name))
        .collect::<Vec<_>>()
        .join("; ")
}

impl From<SourceError> for CodeNexusError {
    fn from(e: SourceError) -> Self {
        match e {
            SourceError::SymbolNotFound { key } => CodeNexusError::NotFound(key),
            SourceError::AmbiguousSymbol { key, candidates } => {
                CodeNexusError::InvalidInput(format!(
                    "ambiguous symbol \"{key}\" matches {} candidates: {}",
                    candidates.len(),
                    candidate_list(&candidates)
                ))
            }
            e @ (SourceError::StaleIndex { .. }
            | SourceError::WindowOutsideSpan { .. }
            | SourceError::SourceTooLarge { .. }) => CodeNexusError::InvalidInput(e.to_string()),
            SourceError::SourceUnavailable(msg) => CodeNexusError::Internal(msg),
        }
    }
}

/// 按标识读取符号完整源码（成功返回）。
/// `start_line`/`end_line` 为实际切片行号（应用窗口后），
/// `total_lines` 为符号完整 span 行数（分页参考）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SymbolSource {
    pub node_id: String,
    pub label: String,
    pub qualified_name: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub total_lines: u32,
    pub source: String,
}

/// 点查命中的节点行（label 由查询的表确定，取 `NodeLabel::table_name`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SymbolRow {
    pub node_id: String,
    pub label: &'static str,
    pub qualified_name: String,
    pub name: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub project: String,
}

/// span-bearing label 集合：运行时从 schema 列清单过滤——列清单须**同时含**
/// startLine / endLine（真 span 符号）与 name / qualifiedName（可寻址符号）。
/// 不硬编码——45 个 label 演进时自动同步。排除项及原因：
/// File/Folder/Module 无 span 列；Variable/GlobalVar/Parameter/Const/Static/
/// TypeAlias/Typedef 仅有单点 startLine 无 endLine（点符号，非"完整源码"
/// 语义）；Embedding 无 name/qualifiedName（辅助向量表，非符号）。
///
/// 结果是 schema 列清单的纯函数，进程内不变——`OnceLock` 缓存一次计算
/// （每次调用重建 45 label × 4 列检查 → 进程内一次）。返回切片，
/// 调用方只读。
pub(crate) fn span_bearing_labels() -> &'static [NodeLabel] {
    static LABELS: std::sync::OnceLock<Vec<NodeLabel>> = std::sync::OnceLock::new();
    LABELS.get_or_init(|| {
        const REQUIRED: [&str; 4] = ["startLine", "endLine", "name", "qualifiedName"];
        NodeLabel::all()
            .into_iter()
            .filter(|l| {
                let cols = node_table_columns(*l);
                REQUIRED.iter().all(|c| cols.contains(c))
            })
            .collect()
    })
}

/// 定位键的 WHERE 谓词（图谱点查键；绝不生成读盘路径）。
/// `escape_cypher_string` 只做内容转义，单引号包裹由本函数负责（house 用法：
/// `index.rs` 的 `WHERE n.project = '{proj}'`）。
fn quoted(s: &str) -> String {
    format!("'{}'", escape_cypher_string(s))
}

fn symbol_where_clause(key: &SymbolKey) -> String {
    match key {
        SymbolKey::QualifiedName(qn) => {
            format!("n.qualifiedName = {}", quoted(qn))
        }
        SymbolKey::NodeId(id) => format!("n.id = {}", quoted(id)),
        SymbolKey::FilePathAndName { file_path, name } => format!(
            "n.filePath = {} AND n.name = {}",
            quoted(file_path),
            quoted(name)
        ),
    }
}

/// 定位符号：逐 span-bearing label 点查，合并三态（0 → NotFound / 1 → 命中 /
/// 多 → Ambiguous）。每表 `LIMIT 2`，凑满 2 行即止——歧义判定不需要全部候选。
/// 与 `index.rs` 的逐 label 顺序查询同款：LadybugDB 异构端点不支持 UNION 于
/// 多 label 模式，顺序点查即 house 模式。
pub(crate) fn resolve_symbol(
    storage: &dyn crate::storage::capability::Storage,
    key: &SymbolKey,
) -> Result<SymbolRow, SourceError> {
    let where_clause = symbol_where_clause(key);
    let mut hits: Vec<SymbolRow> = Vec::new();
    for label in span_bearing_labels() {
        let cypher = format!(
            "MATCH (n:{}) WHERE {} \
             AND n.startLine IS NOT NULL AND n.endLine IS NOT NULL \
             RETURN n.id AS id, n.qualifiedName AS qualifiedName, n.name AS name, \
             n.filePath AS filePath, n.startLine AS startLine, n.endLine AS endLine, \
             n.project AS project \
             LIMIT 2;",
            escape_identifier(label.table_name()),
            where_clause
        );
        let rows = storage
            .query(&cypher)
            .map_err(|e| SourceError::SourceUnavailable(e.to_string()))?;
        for row in &rows {
            hits.push(parse_symbol_row(*label, row)?);
        }
        if hits.len() >= 2 {
            break;
        }
    }

    match hits.len() {
        0 => Err(SourceError::SymbolNotFound {
            key: key.describe(),
        }),
        1 => Ok(hits.into_iter().next().expect("len checked == 1")),
        _ => Err(SourceError::AmbiguousSymbol {
            key: key.describe(),
            candidates: hits
                .into_iter()
                .map(|r| CandidateRef {
                    node_id: r.node_id,
                    label: r.label.to_string(),
                    qualified_name: r.qualified_name,
                })
                .collect(),
        }),
    }
}

/// 行向量 → 结构化行。列序与 [`resolve_symbol`] 的 RETURN 子句一一对应；
/// NULL span 已被 WHERE 过滤，此处遇到坏值按环境故障上报（不静默吞）。
fn parse_symbol_row(label: NodeLabel, row: &[serde_json::Value]) -> Result<SymbolRow, SourceError> {
    let as_string = |i: usize| -> Result<String, SourceError> {
        row.get(i)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| {
                SourceError::SourceUnavailable(format!(
                    "query row column {i} is not a string: {:?}",
                    row.get(i)
                ))
            })
    };
    let as_u32 = |i: usize| -> Result<u32, SourceError> {
        row.get(i)
            .and_then(|v| v.as_u64())
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| {
                SourceError::SourceUnavailable(format!(
                    "query row column {i} is not a u32: {:?}",
                    row.get(i)
                ))
            })
    };
    Ok(SymbolRow {
        node_id: as_string(0)?,
        qualified_name: as_string(1)?,
        name: as_string(2)?,
        file_path: as_string(3)?,
        start_line: as_u32(4)?,
        end_line: as_u32(5)?,
        project: as_string(6)?,
        label: label.table_name(),
    })
}

/// File 行的 freshness 依 据：hash 与 lineCount
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileFreshness {
    hash: String,
    line_count: u32,
}

/// 查 File 行的 hash 与 lineCount（`get_file_hash` 不带 lineCount，故直接点查）
fn query_file_freshness(
    storage: &dyn crate::storage::capability::Storage,
    file_path: &str,
    project: &str,
) -> Result<FileFreshness, SourceError> {
    let cypher = format!(
        "MATCH (f:File) WHERE f.filePath = {} AND f.project = {} \
         RETURN f.hash AS hash, f.lineCount AS lineCount;",
        quoted(file_path),
        quoted(project)
    );
    let rows = storage
        .query(&cypher)
        .map_err(|e| SourceError::SourceUnavailable(e.to_string()))?;
    let row = rows.first().ok_or_else(|| SourceError::StaleIndex {
        file_path: file_path.to_string(),
        reason: StaleReason::FileMissing,
    })?;
    let hash = row
        .first()
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            SourceError::SourceUnavailable(format!(
                "File row for {file_path} has no hash column value"
            ))
        })?;
    let line_count = row
        .get(1)
        .and_then(|v| v.as_u64())
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| {
            SourceError::SourceUnavailable(format!(
                "File row for {file_path} has no lineCount value"
            ))
        })?;
    Ok(FileFreshness { hash, line_count })
}

/// 查项目根路径并 canonicalize（后续切片的穿越防护基准根）
fn project_root(
    storage: &dyn crate::storage::capability::Storage,
    project: &str,
) -> Result<PathBuf, SourceError> {
    let cypher = format!(
        "MATCH (p:Project) WHERE p.id = {} RETURN p.rootPath AS rootPath;",
        quoted(project)
    );
    let rows = storage
        .query(&cypher)
        .map_err(|e| SourceError::SourceUnavailable(e.to_string()))?;
    let root = rows
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| SourceError::StaleIndex {
            file_path: project.to_string(),
            reason: StaleReason::FileMissing,
        })?;
    let path = Path::new(root);
    if !path.is_absolute() {
        return Err(SourceError::StaleIndex {
            file_path: root.to_string(),
            reason: StaleReason::FileMissing,
        });
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| SourceError::StaleIndex {
        file_path: root.to_string(),
        reason: StaleReason::FileMissing,
    })?;
    // 纵深防御：rootPath 退化为文件系统根时 `starts_with` 恒真，后续全部
    // 根包含校验失效。只可能源于被篡改/损坏的索引库——拒绝并指向重索引。
    // 判定用「无父目录」而非 `== "/"`：Windows 盘符根（`C:\`）与 UNC 共享
    // 根 canonicalize 后同样无 parent，字符串等值只覆盖 Unix。
    if canonical.parent().is_none() {
        return Err(SourceError::StaleIndex {
            file_path: root.to_string(),
            reason: StaleReason::InvalidProjectRoot,
        });
    }
    Ok(canonical)
}

/// stale 硬错误校验链（顺序执行，任一失败即拒绝返回源码）的成功产物：
/// 为 hash 一次性读取的文件原始字节。切片直接复用——单次调用至多一次
/// 整文件读 + 一次文件 canonicalize（在链内完成，根 canonicalize 另计）。
#[derive(Debug)]
pub(crate) struct FreshSource {
    /// 索引期同款 BLAKE3 在字节上计算（与读盘路径无关）；切片亦复用。
    pub content: Vec<u8>,
}

/// stale 硬错误校验链（顺序执行，任一失败即拒绝返回源码；成功返回为 hash
/// 一次性读取的文件内容——切片复用该字节，不再二次 canonicalize/读盘）：
/// ① Project 根存在且可 canonicalize（退化根 `/` 拒绝）→ 否则 FileMissing
/// ② File 行存在（hash/lineCount 齐备）→ 否则 FileMissing
/// ③ 文件 canonicalize + 根包含校验 → 否则 FileMissing
/// ④ 有界整读（索引期同款 `MAX_FILE_SIZE` 上限，读取前检查）+ BLAKE3 比对
///    → 不等则 HashMismatch
/// ⑤ 节点 span 不超文件实际长度 → 否则 SpanBeyondFile
pub(crate) fn validate_freshness(
    storage: &dyn crate::storage::capability::Storage,
    row: &SymbolRow,
) -> Result<FreshSource, SourceError> {
    let root = project_root(storage, &row.project)?;

    let file = query_file_freshness(storage, &row.file_path, &row.project)?;

    // DB filePath 是项目根相对路径（discover::walker 语义，前导 '/' 剥除）；
    // hash 解析走与切片读取同款的 canonicalize + 根包含校验，防 DB 行被篡改
    // 时 `../` 逃出项目根对任意可读文件做存在性/内容探测。
    let abs = root.join(row.file_path.trim_start_matches('/'));
    let canonical_file = abs.canonicalize().map_err(|_| SourceError::StaleIndex {
        file_path: row.file_path.clone(),
        reason: StaleReason::FileMissing,
    })?;
    if !canonical_file.starts_with(&root) {
        return Err(SourceError::StaleIndex {
            file_path: row.file_path.clone(),
            reason: StaleReason::FileMissing,
        });
    }

    // 有界整读：hash 与切片共用这一次读取。上限检查落在已打开的句柄上
    // （fstat 同一对象）并以 `take` 封顶，消除 stat/read 竞态窗口；错误映射
    // 与 `compute_file_hash` 的调用方约定一致（InvalidInput → FileMissing）。
    // canonical 路径不可能再是符号链接，symlink 拒绝由此天然成立。
    let handle = std::fs::File::open(&canonical_file).map_err(|_| SourceError::StaleIndex {
        file_path: row.file_path.clone(),
        reason: StaleReason::FileMissing,
    })?;
    if handle
        .metadata()
        .map_err(|_| SourceError::StaleIndex {
            file_path: row.file_path.clone(),
            reason: StaleReason::FileMissing,
        })?
        .len()
        > MAX_FILE_SIZE
    {
        return Err(SourceError::StaleIndex {
            file_path: row.file_path.clone(),
            reason: StaleReason::FileMissing,
        });
    }
    let mut content = Vec::new();
    use std::io::Read as _;
    handle
        .take(MAX_FILE_SIZE)
        .read_to_end(&mut content)
        .map_err(|_| SourceError::StaleIndex {
            file_path: row.file_path.clone(),
            reason: StaleReason::FileMissing,
        })?;
    let actual_hash = compute_content_hash(&content);
    if actual_hash != file.hash {
        return Err(SourceError::StaleIndex {
            file_path: row.file_path.clone(),
            reason: StaleReason::HashMismatch,
        });
    }

    if row.end_line > file.line_count {
        return Err(SourceError::StaleIndex {
            file_path: row.file_path.clone(),
            reason: StaleReason::SpanBeyondFile,
        });
    }

    Ok(FreshSource { content })
}

/// 计算有效切片范围：span ∩ window。
/// 返回 (eff_start, eff_end, total_lines)；total_lines 为符号完整 span 行数
/// （分页参考，不随窗口收缩）。
///
/// 窗口语义：
/// - `window.start` 必须落在 span 内（否则 [`SourceError::WindowOutsideSpan`]）
/// - `window.end` 省略 = 到 span 尾；超出 span 尾则 clamp 到 span 尾
/// - `window.end < window.start` 属调用方 bug，防御性 clamp 为单行（start）
pub(crate) fn effective_range(
    row: &SymbolRow,
    opts: &SourceOptions,
) -> Result<(u32, u32, u32), SourceError> {
    let (span_start, span_end) = (row.start_line, row.end_line);
    if span_end < span_start {
        // DB 行 span 倒置 = 行数据已损坏/过期；直接拒绝，避免 u32 下溢。
        return Err(SourceError::StaleIndex {
            file_path: row.file_path.clone(),
            reason: StaleReason::SpanBeyondFile,
        });
    }
    let total_lines = span_end - span_start + 1;
    let (eff_start, eff_end) = match opts.window {
        None => (span_start, span_end),
        Some(w) => {
            if w.start < span_start || w.start > span_end {
                return Err(SourceError::WindowOutsideSpan {
                    start: w.start,
                    span_start,
                    span_end,
                });
            }
            let end = w.end.unwrap_or(span_end).clamp(w.start, span_end);
            (w.start, end)
        }
    };
    let count = (eff_end - eff_start + 1) as usize;
    if count > opts.max_lines.get() {
        return Err(SourceError::SourceTooLarge {
            total_lines: count,
            max_lines: opts.max_lines.get(),
        });
    }
    Ok((eff_start, eff_end, total_lines))
}

/// 按定位键读取符号完整源码片段（core，可测；CLI/MCP wrapper 见下方 forge 定义）。
///
/// 流程：定位（[`resolve_symbol`]）→ stale 硬错误校验（[`validate_freshness`]，
/// 任一失败拒绝返回）→ 窗口与上限（[`effective_range`]）→
/// 穿越防护切片（`read_source_slice_canonical`，读盘路径只取 DB 节点行——
/// **图谱是唯一路径真源**，入参 filePath 仅作查询键）。
///
/// 同步实现：storage 查询为同步 API（`run_search` 同款 house 形状）。
pub fn run_read_symbol_source(
    kit: &AsyncKit<AsyncReady>,
    key: SymbolKey,
    opts: &SourceOptions,
) -> Result<SymbolSource, SourceError> {
    let storage = kit
        .require::<StorageModule>()
        .map_err(|e| SourceError::SourceUnavailable(e.to_string()))?;
    let row = resolve_symbol(&*storage, &key)?;
    // 校验链顺带完成唯一的整文件读取：切片复用其字节——单次调用至多一次
    // 整文件读 + 一次文件 canonicalize（根 canonicalize 另计）。
    let fresh = validate_freshness(&*storage, &row)?;
    let (eff_start, eff_end, total_lines) = effective_range(&row, opts)?;
    // 非 UTF-8 文件：hash 按字节计算不受影响；切片需要文本，错误族与消息
    // 与旧切片读取失败路径保持一致。
    let content = String::from_utf8(fresh.content).map_err(|_| {
        SourceError::SourceUnavailable(format!(
            "failed to slice {} lines [{eff_start}, {eff_end}] under canonical project root",
            row.file_path
        ))
    })?;
    let source = slice_lines(&content, eff_start, eff_end).ok_or_else(|| {
        SourceError::SourceUnavailable(format!(
            "failed to slice {} lines [{eff_start}, {eff_end}] under canonical project root",
            row.file_path
        ))
    })?;
    Ok(SymbolSource {
        node_id: row.node_id,
        label: row.label.to_string(),
        qualified_name: row.qualified_name,
        file_path: row.file_path,
        start_line: eff_start,
        end_line: eff_end,
        total_lines,
        source,
    })
}

/// CLI 参数 → 定位键（三键互斥；零键或双键给 usage 错误）。
/// 空串 = 未提供（sdforge sentinel 默认值的 house 约定）。
#[cfg(any(feature = "cli", feature = "mcp", test))]
fn build_key_from_cli(
    qualified_name: &str,
    id: &str,
    file: &str,
    name: &str,
) -> Result<SymbolKey, CodeNexusError> {
    let qn = !qualified_name.trim().is_empty();
    let idv = !id.trim().is_empty();
    let file_v = !file.trim().is_empty();
    let name_v = !name.trim().is_empty();
    let mut given: Vec<&str> = Vec::new();
    if qn {
        given.push("--qualified_name");
    }
    if idv {
        given.push("--id");
    }
    if file_v || name_v {
        given.push("--file/--name 组合");
    }
    let usage = "usage: --qualified_name <qn> | --id <node-id> | (--file <path> --name <name>) — 三键互斥，恰好给出一个";
    if given.len() > 1 {
        return Err(CodeNexusError::InvalidInput(format!(
            "multiple locator keys given ({}): {usage}",
            given.join(" + ")
        )));
    }
    if qn {
        return Ok(SymbolKey::QualifiedName(qualified_name.trim().to_string()));
    }
    if idv {
        return Ok(SymbolKey::NodeId(id.trim().to_string()));
    }
    if file_v && name_v {
        return Ok(SymbolKey::FilePathAndName {
            file_path: file.trim().to_string(),
            name: name.trim().to_string(),
        });
    }
    if given.len() == 1 {
        return Err(CodeNexusError::InvalidInput(
            "--file 与 --name 必须成对给出".to_string(),
        ));
    }
    Err(CodeNexusError::InvalidInput(format!(
        "no locator key given: {usage}"
    )))
}

/// CLI wrapper — `codenexus source`：JSON 输出（house 风格）。
/// 可选参数经 main.rs `SENTINEL_DEFAULTS` 注入默认值（start/end_line 0 =
/// 未提供；max_lines 0 = 默认 500）。
#[cfg(any(feature = "cli", feature = "mcp", test))]
#[forge::log(args, err_detail)]
#[forge(
    name = "source",
    version = "0.3.5",
    description = "Read the full source text of a span-bearing symbol from the indexed graph. Locator keys (mutually exclusive): --qualified-name, --id, or --file + --name. Optional: --start-line/--end-line (absolute line window), --max-lines (cap, default 500). Refuses with a stale-index error when the file changed after indexing (run: codenexus index).",
    cli = true
)]
async fn source(
    qualified_name: String,
    id: String,
    file: String,
    name: String,
    start_line: u32,
    end_line: u32,
    max_lines: u32,
) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let key = build_key_from_cli(&qualified_name, &id, &file, &name)
        .map_err(|e| to_api_error(e, "source_error"))?;
    let opts = cli_options(start_line, end_line, max_lines);
    let result = run_read_symbol_source(&kit, key, &opts)
        .map_err(|e| to_api_error(e.into(), "source_error"))?;
    let json = serde_json::to_string(&result)
        .map_err(|e| to_api_error(CodeNexusError::from(e), "source_error"))?;
    println!("{json}");
    Ok(())
}

/// CLI 数值参数 → [`SourceOptions`]（0 = 未提供/默认的 sentinel 约定）
#[cfg(any(feature = "cli", feature = "mcp", test))]
fn cli_options(start_line: u32, end_line: u32, max_lines: u32) -> SourceOptions {
    SourceOptions {
        window: if start_line == 0 {
            None
        } else {
            Some(LineWindow {
                start: start_line,
                end: if end_line == 0 { None } else { Some(end_line) },
            })
        },
        max_lines: NonZeroUsize::new(max_lines as usize)
            .unwrap_or_else(|| SourceOptions::default().max_lines),
    }
}

/// MCP wrapper — returns result for MCP protocol. 返回值类型与 core 相同
/// （`SymbolSource`），故与 CLI core 天然同构。
#[cfg(feature = "mcp")]
#[forge(
    name = "source",
    version = "0.3.5",
    tool_name = "source",
    description = "Read the full source text of a span-bearing symbol (function/method/class/struct/trait/interface/impl/macro/endpoint/route/...) from the indexed graph. Locator keys (mutually exclusive): qualified_name (stable across re-index; copy it from search/context output), id (node UUID primary key, invalid after re-index), or file+name pair. Optional: start_line/end_line (absolute 1-based line window for paging large symbols), max_lines (cap, default 500, 0 = default). Stale hard error: if the file changed after indexing, the tool refuses and asks to re-run: codenexus index. Typical agent chain: search -> copy qualifiedName -> source."
)]
async fn source_mcp(
    qualified_name: String,
    id: String,
    file: String,
    name: String,
    start_line: u32,
    end_line: u32,
    max_lines: u32,
) -> Result<SymbolSource, ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let key = build_key_from_cli(&qualified_name, &id, &file, &name)
        .map_err(|e| to_api_error(e, "source_error"))?;
    let opts = cli_options(start_line, end_line, max_lines);
    run_read_symbol_source(&kit, key, &opts).map_err(|e| to_api_error(e.into(), "source_error"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::hash::compute_file_hash;

    #[test]
    fn symbol_source_serializes_camel_case_fields() {
        let s = SymbolSource {
            node_id: "n1".into(),
            label: "Function".into(),
            qualified_name: "demo.f".into(),
            file_path: "/src/a.rs".into(),
            start_line: 10,
            end_line: 20,
            total_lines: 11,
            source: "fn f() {}".into(),
        };
        let v = serde_json::to_value(&s).expect("serialize");
        for k in [
            "nodeId",
            "label",
            "qualifiedName",
            "filePath",
            "startLine",
            "endLine",
            "totalLines",
            "source",
        ] {
            assert!(v.get(k).is_some(), "missing camelCase field {k}");
        }
        assert!(v.get("node_id").is_none(), "snake_case must not leak");
    }

    #[test]
    fn source_options_default_max_lines_is_500() {
        assert_eq!(DEFAULT_MAX_LINES, 500);
        assert_eq!(SourceOptions::default().max_lines.get(), 500);
        assert!(SourceOptions::default().window.is_none());
    }

    #[test]
    fn source_error_display_carries_next_step_hint() {
        let e = SourceError::StaleIndex {
            file_path: "/a.rs".into(),
            reason: StaleReason::HashMismatch,
        };
        let msg = e.to_string();
        assert!(msg.contains("codenexus index"), "{msg}");

        let e2 = SourceError::SourceTooLarge {
            total_lines: 2000,
            max_lines: 500,
        };
        let msg2 = e2.to_string();
        assert!(msg2.contains("2000") && msg2.contains("500"), "{msg2}");
        assert!(msg2.contains("window"), "must hint window paging: {msg2}");
    }

    #[test]
    fn ambiguous_symbol_error_lists_candidates() {
        let e = SourceError::AmbiguousSymbol {
            key: "demo.f".into(),
            candidates: vec![
                CandidateRef {
                    node_id: "a".into(),
                    label: "Function".into(),
                    qualified_name: "demo.f".into(),
                },
                CandidateRef {
                    node_id: "b".into(),
                    label: "Method".into(),
                    qualified_name: "m.f".into(),
                },
            ],
        };
        let msg = e.to_string();
        assert!(msg.contains("demo.f"), "{msg}");
        assert!(msg.contains("Function") && msg.contains("Method"), "{msg}");
    }

    #[test]
    fn source_error_maps_to_unified_exit_codes() {
        let not_found: CodeNexusError = SourceError::SymbolNotFound { key: "x".into() }.into();
        assert_eq!(not_found.exit_code(), 4);

        let stale: CodeNexusError = SourceError::StaleIndex {
            file_path: "/a.rs".into(),
            reason: StaleReason::FileMissing,
        }
        .into();
        assert_eq!(stale.exit_code(), 2);

        let internal: CodeNexusError = SourceError::SourceUnavailable("db closed".into()).into();
        assert_eq!(internal.exit_code(), 1);
    }

    // ── 解析链测试基建（对齐 search.rs 的 house 助手形状）──

    use crate::kit::{build_kit, KitBootstrapConfig};
    use crate::storage::capability::Storage;
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_source_testdb");
        (dir, path)
    }

    fn build_kit_for_db(db: &std::path::Path) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    fn seed_project(storage: &dyn Storage, id: &str, name: &str) {
        storage
            .execute(&format!(
                "CREATE (:Project {{id: '{id}', name: '{name}', rootPath: '/demo', language: 'rust', fileCount: 1, indexedAt: 1000, lastCommit: 'abc'}});"
            ))
            .expect("create project");
    }

    fn storage_of(kit: &AsyncKit<AsyncReady>) -> std::sync::Arc<dyn Storage> {
        kit.require::<StorageModule>().expect("storage module")
    }

    #[test]
    fn span_bearing_labels_exclude_spanless_tables() {
        let labels = span_bearing_labels();
        let names: Vec<&str> = labels.iter().map(|l| l.table_name()).collect();
        for excluded in [
            "File",     // 无 span 列
            "Folder",   // 无 span 列
            "Module",   // 无 span 列
            "Variable", // 仅单点 startLine 无 endLine
            "GlobalVar",
            "Parameter",
            "Const",
            "Static",
            "TypeAlias",
            "Typedef",
            "Embedding", // 无 name/qualifiedName 的辅助向量表
        ] {
            assert!(
                !names.contains(&excluded),
                "{excluded} must not be span-bearing-addressable"
            );
        }
        for included in ["Function", "Method", "Class"] {
            assert!(names.contains(&included), "{names:?}");
        }
    }

    #[test]
    fn resolve_by_qualified_name_hits_unique_function() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = storage_of(&kit);
        seed_project(&*storage, "demo", "demo");
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'do_thing', qualifiedName: 'demo.do_thing', filePath: '/src/a.rs', startLine: 10, endLine: 20, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create function");

        let row = resolve_symbol(&*storage, &SymbolKey::QualifiedName("demo.do_thing".into()))
            .expect("unique hit");
        assert_eq!(row.node_id, "f1");
        assert_eq!(row.label, "Function");
        assert_eq!(row.name, "do_thing");
        assert_eq!(row.file_path, "/src/a.rs");
        assert_eq!(row.start_line, 10);
        assert_eq!(row.end_line, 20);
    }

    #[test]
    fn resolve_unknown_qualified_name_is_not_found() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = storage_of(&kit);
        seed_project(&*storage, "demo", "demo");

        let err = resolve_symbol(&*storage, &SymbolKey::QualifiedName("demo.ghost".into()))
            .expect_err("no seed row");
        assert!(
            matches!(err, SourceError::SymbolNotFound { ref key } if key.contains("qualifiedName=demo.ghost")),
            "{err}"
        );
    }

    #[test]
    fn resolve_duplicate_qualified_name_reports_ambiguity() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = storage_of(&kit);
        seed_project(&*storage, "demo", "demo");
        for id in ["f_dup_a", "f_dup_b"] {
            storage.execute(&format!("CREATE (:Function {{id: '{id}', project: 'demo', name: 'dup_fn', qualifiedName: 'demo.dup_fn', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''}});")).expect("create duplicate function");
        }

        let err = resolve_symbol(&*storage, &SymbolKey::QualifiedName("demo.dup_fn".into()))
            .expect_err("two rows share one qualifiedName");
        assert!(
            matches!(err, SourceError::AmbiguousSymbol { ref candidates, .. } if candidates.len() == 2),
            "{err}"
        );
    }

    #[test]
    fn resolve_by_node_id_hits_unique_row() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = storage_of(&kit);
        seed_project(&*storage, "demo", "demo");
        storage.execute("CREATE (:Function {id: 'uniq_id_1', project: 'demo', name: 'id_hit', qualifiedName: 'demo.id_hit', filePath: '/src/b.rs', startLine: 3, endLine: 9, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create function");

        let row =
            resolve_symbol(&*storage, &SymbolKey::NodeId("uniq_id_1".into())).expect("unique hit");
        assert_eq!(row.node_id, "uniq_id_1");
        assert_eq!(row.label, "Function");
        assert_eq!(row.qualified_name, "demo.id_hit");
    }

    #[test]
    fn resolve_duplicate_node_id_across_tables_reports_ambiguity() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = storage_of(&kit);
        seed_project(&*storage, "demo", "demo");
        storage.execute("CREATE (:Function {id: 'dup_id', project: 'demo', name: 'cross_a', qualifiedName: 'demo.cross_a', filePath: '/src/a.rs', startLine: 1, endLine: 4, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create function");
        storage.execute("CREATE (:Method {id: 'dup_id', project: 'demo', name: 'cross_b', qualifiedName: 'demo.cross_b', filePath: '/src/a.rs', startLine: 6, endLine: 10, signature: '', returnType: '', isExported: false, docstring: '', content: '', parameterCount: 0, parentQn: ''});").expect("create method");

        let err = resolve_symbol(&*storage, &SymbolKey::NodeId("dup_id".into()))
            .expect_err("same id in two tables");
        assert!(
            matches!(err, SourceError::AmbiguousSymbol { ref candidates, .. }
                if candidates.len() == 2
                && candidates.iter().any(|c| c.label == "Function")
                && candidates.iter().any(|c| c.label == "Method")),
            "{err}"
        );
    }

    #[test]
    fn resolve_by_file_and_name_hits_unique_row() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = storage_of(&kit);
        seed_project(&*storage, "demo", "demo");
        storage.execute("CREATE (:Function {id: 'fn_fp', project: 'demo', name: 'combo_fn', qualifiedName: 'demo.combo_fn', filePath: '/src/combo.rs', startLine: 2, endLine: 8, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").expect("create function");

        let row = resolve_symbol(
            &*storage,
            &SymbolKey::FilePathAndName {
                file_path: "/src/combo.rs".into(),
                name: "combo_fn".into(),
            },
        )
        .expect("unique hit");
        assert_eq!(row.node_id, "fn_fp");
        assert_eq!(row.start_line, 2);
    }

    #[test]
    fn resolve_by_file_and_name_multiple_hits_reports_ambiguity() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = storage_of(&kit);
        seed_project(&*storage, "demo", "demo");
        for id in ["fn_c1", "fn_c2"] {
            storage.execute(&format!("CREATE (:Function {{id: '{id}', project: 'demo', name: 'same_name', qualifiedName: 'demo.same_name#{id}', filePath: '/src/same.rs', startLine: 1, endLine: 4, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''}});")).expect("create function");
        }

        let err = resolve_symbol(
            &*storage,
            &SymbolKey::FilePathAndName {
                file_path: "/src/same.rs".into(),
                name: "same_name".into(),
            },
        )
        .expect_err("two rows share filePath+name");
        assert!(
            matches!(err, SourceError::AmbiguousSymbol { ref candidates, .. } if candidates.len() == 2),
            "{err}"
        );
    }

    // ── stale 硬错误链测试 ──

    /// 索引场景种子：真实临时目录 + 真实 BLAKE3 hash + File 行 + Function 行。
    /// `project_root` 允许指向不存在路径（测试 ① 根缺失分支）。
    /// 返回 (kit, dir, 源文件绝对路径字符串)；storage 经 `storage_of(&kit)` 取。
    fn seed_fresh_workspace_at(
        content: &str,
        line_count: u32,
        project_root_override: Option<&std::path::Path>,
    ) -> (AsyncKit<AsyncReady>, TempDir, String) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        // house 语义：DB filePath 为项目根相对路径（discover::walker）
        let file_path = root.join("a.rs");
        std::fs::write(&file_path, content).expect("write source file");
        let file_path_str = "a.rs".to_string();
        let hash = compute_file_hash(&file_path).expect("hash");
        let db = dir.path().canonicalize().unwrap().join("db");

        let kit = build_kit_for_db(&db);
        let storage = storage_of(&kit);
        let root_str = project_root_override
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| root.to_string_lossy().to_string());
        storage.execute(&format!(
            "CREATE (:Project {{id: 'demo', name: 'demo', rootPath: '{}', language: 'rust', fileCount: 1, indexedAt: 1000, lastCommit: 'abc'}});",
            root_str.replace('\'', "\\'")
        )).expect("create project");
        storage.execute(&format!(
            "CREATE (:File {{id: 'file_1', project: 'demo', name: 'a.rs', filePath: '{}', language: 'rust', hash: '{}', lineCount: {line_count}}});",
            file_path_str.replace('\'', "\\'"),
            hash
        )).expect("create file row");
        storage.execute(&format!(
            "CREATE (:Function {{id: 'fn_1', project: 'demo', name: 'seed_fn', qualifiedName: 'demo.seed_fn', filePath: '{}', startLine: 1, endLine: 3, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''}});",
            file_path_str.replace('\'', "\\'")
        )).expect("create function");
        (kit, dir, file_path_str)
    }

    fn seed_fresh_workspace(
        content: &str,
        line_count: u32,
    ) -> (AsyncKit<AsyncReady>, TempDir, String) {
        seed_fresh_workspace_at(content, line_count, None)
    }

    const SEED_SOURCE: &str = "fn seed_fn() {\n    body\n}\n";

    /// DB Function 行 filePath 被篡改为 `../` 逃逸路径且目标真实存在时，
    /// hash 读取必须在 canonicalize + 根包含校验处拒绝（与切片读取同款
    /// 防护），不得对项目根外任意可读文件做存在性/内容探测。
    #[test]
    fn freshness_rejects_traversal_file_path() {
        let (kit, dir, _fp) = seed_fresh_workspace(SEED_SOURCE, 3);
        let storage = storage_of(&kit);
        // 逃逸目标真实存在（临时目录的父目录在测试进程控制内）
        let escape_target = dir
            .path()
            .parent()
            .unwrap()
            .join("freshness_escape_probe.rs");
        std::fs::write(&escape_target, "probe").expect("write escape target");
        storage.execute(&format!(
            "CREATE (:Function {{id: 'fn_esc', project: 'demo', name: 'esc_fn', qualifiedName: 'demo.esc_fn', filePath: '../freshness_escape_probe.rs', startLine: 1, endLine: 3, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''}});",
        ))
        .expect("create escaping function");
        let row = resolve_symbol(&*storage, &SymbolKey::QualifiedName("demo.esc_fn".into()))
            .expect("resolve");
        let err = validate_freshness(&*storage, &row).expect_err("escape path rejected");
        assert!(
            matches!(
                err,
                SourceError::StaleIndex {
                    reason: StaleReason::FileMissing,
                    ..
                }
            ),
            "guard must reject before hash probe: {err}"
        );
        let _ = std::fs::remove_file(&escape_target);
    }

    #[test]
    fn freshness_ok_reads_indexed_file_bytes() {
        let (kit, dir, _fp) = seed_fresh_workspace(SEED_SOURCE, 3);
        let storage = storage_of(&kit);
        let row = resolve_symbol(&*storage, &SymbolKey::QualifiedName("demo.seed_fn".into()))
            .expect("resolve");
        let fresh = validate_freshness(&*storage, &row).expect("fresh workspace");
        let expected = std::fs::read(dir.path().join("a.rs")).expect("read indexed file");
        assert_eq!(
            fresh.content, expected,
            "the validation chain's single read returns the indexed file's bytes"
        );
    }

    #[test]
    fn freshness_missing_project_root_is_file_missing() {
        let (kit, _dir, _fp) =
            seed_fresh_workspace_at(SEED_SOURCE, 3, Some(Path::new("/nonexistent-root-xyz")));
        let storage = storage_of(&kit);
        let row = resolve_symbol(&*storage, &SymbolKey::QualifiedName("demo.seed_fn".into()))
            .expect("resolve");
        let err = validate_freshness(&*storage, &row).expect_err("root gone");
        assert!(
            matches!(
                err,
                SourceError::StaleIndex {
                    reason: StaleReason::FileMissing,
                    ..
                }
            ),
            "{err}"
        );
    }

    /// 纵深防御：rootPath 为文件系统根 `/` 时 `starts_with` 恒真，穿越防护
    /// 完全失效（任何盘上路径都被判为「根内」）。正常索引不可能写出这种根，
    /// 只会源于被篡改/损坏的索引库——必须拒绝，且错误指向重索引。
    #[test]
    fn freshness_rejects_degenerate_filesystem_root() {
        let (kit, _dir, _fp) = seed_fresh_workspace_at(SEED_SOURCE, 3, Some(Path::new("/")));
        let storage = storage_of(&kit);
        let row = resolve_symbol(&*storage, &SymbolKey::QualifiedName("demo.seed_fn".into()))
            .expect("resolve");
        let err = validate_freshness(&*storage, &row).expect_err("degenerate root rejected");
        assert!(
            matches!(
                err,
                SourceError::StaleIndex {
                    reason: StaleReason::InvalidProjectRoot,
                    ..
                }
            ),
            "degenerate root must carry its own stale reason: {err}"
        );
        assert!(
            err.to_string().contains("codenexus index"),
            "error must point at the re-index remedy: {err}"
        );
    }

    #[test]
    fn freshness_modified_file_is_hash_mismatch_with_reindex_hint() {
        let (kit, dir, _fp) = seed_fresh_workspace(SEED_SOURCE, 3);
        std::fs::write(
            dir.path().join("a.rs"),
            "fn seed_fn() {\n    CHANGED body\n}\n",
        )
        .expect("modify file");
        let storage = storage_of(&kit);
        let row = resolve_symbol(&*storage, &SymbolKey::QualifiedName("demo.seed_fn".into()))
            .expect("resolve");
        let err = validate_freshness(&*storage, &row).expect_err("content changed");
        assert!(
            matches!(
                err,
                SourceError::StaleIndex {
                    reason: StaleReason::HashMismatch,
                    ..
                }
            ),
            "{err}"
        );
        assert!(err.to_string().contains("codenexus index"), "{err}");
    }

    #[test]
    fn freshness_deleted_file_is_file_missing() {
        let (kit, dir, _fp) = seed_fresh_workspace(SEED_SOURCE, 3);
        std::fs::remove_file(dir.path().join("a.rs")).expect("delete file");
        let storage = storage_of(&kit);
        let row = resolve_symbol(&*storage, &SymbolKey::QualifiedName("demo.seed_fn".into()))
            .expect("resolve");
        let err = validate_freshness(&*storage, &row).expect_err("file gone");
        assert!(
            matches!(
                err,
                SourceError::StaleIndex {
                    reason: StaleReason::FileMissing,
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn freshness_span_beyond_file_length_is_rejected() {
        // 内容与 hash 一致（③ 通过），但 File 行 lineCount=2 < Function endLine=3
        // —— 单独验证 ④ 行边界检查
        let (kit, _dir, _fp) = seed_fresh_workspace(SEED_SOURCE, 2);
        let storage = storage_of(&kit);
        let row = resolve_symbol(&*storage, &SymbolKey::QualifiedName("demo.seed_fn".into()))
            .expect("resolve");
        let err = validate_freshness(&*storage, &row).expect_err("span exceeds file");
        assert!(
            matches!(
                err,
                SourceError::StaleIndex {
                    reason: StaleReason::SpanBeyondFile,
                    ..
                }
            ),
            "{err}"
        );
    }

    // ── 窗口与上限测试（纯逻辑，直接构造 SymbolRow）──

    fn row_span(start: u32, end: u32) -> SymbolRow {
        SymbolRow {
            node_id: "n1".into(),
            label: "Function",
            qualified_name: "demo.big".into(),
            name: "big".into(),
            file_path: "/src/big.rs".into(),
            start_line: start,
            end_line: end,
            project: "demo".into(),
        }
    }

    #[test]
    fn oversized_symbol_without_window_is_too_large() {
        let row = row_span(1, 2000);
        let err = effective_range(&row, &SourceOptions::default())
            .expect_err("2000 lines exceed default 500");
        assert!(
            matches!(
                err,
                SourceError::SourceTooLarge {
                    total_lines: 2000,
                    max_lines: 500
                }
            ),
            "{err}"
        );
        assert!(
            err.to_string().contains("window"),
            "must hint paging: {err}"
        );
    }

    #[test]
    fn window_start_outside_span_is_rejected() {
        let row = row_span(10, 20);
        let err = effective_range(
            &row,
            &SourceOptions {
                window: Some(LineWindow {
                    start: 5,
                    end: Some(15),
                }),
                ..SourceOptions::default()
            },
        )
        .expect_err("start 5 below span");
        assert!(
            matches!(
                err,
                SourceError::WindowOutsideSpan {
                    start: 5,
                    span_start: 10,
                    span_end: 20
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn window_intersect_returns_effective_range() {
        let row = row_span(10, 20);
        let (s, e, total) = effective_range(
            &row,
            &SourceOptions {
                window: Some(LineWindow {
                    start: 12,
                    end: Some(15),
                }),
                ..SourceOptions::default()
            },
        )
        .expect("window inside span");
        assert_eq!((s, e, total), (12, 15, 11));
    }

    #[test]
    fn window_end_omitted_extends_to_span_tail() {
        let row = row_span(10, 20);
        let (s, e, _total) = effective_range(
            &row,
            &SourceOptions {
                window: Some(LineWindow {
                    start: 15,
                    end: None,
                }),
                ..SourceOptions::default()
            },
        )
        .expect("open-ended window");
        assert_eq!((s, e), (15, 20));
    }

    #[test]
    fn window_end_beyond_span_clamps_to_span_tail() {
        let row = row_span(10, 20);
        let (s, e, _total) = effective_range(
            &row,
            &SourceOptions {
                window: Some(LineWindow {
                    start: 15,
                    end: Some(99),
                }),
                ..SourceOptions::default()
            },
        )
        .expect("end clamped");
        assert_eq!((s, e), (15, 20));
    }

    #[test]
    fn full_span_within_limit_passes() {
        let row = row_span(1, 100);
        let (s, e, total) =
            effective_range(&row, &SourceOptions::default()).expect("100 lines fit 500");
        assert_eq!((s, e, total), (1, 100, 100));
    }

    /// span 倒置（end < start）= DB 行已损坏/过期：入口直接拒绝，
    /// 不得走到 `span_end - span_start + 1` 的 u32 下溢（debug panic）。
    #[test]
    fn effective_range_rejects_inverted_span() {
        let row = row_span(5, 2);
        let err = effective_range(&row, &SourceOptions::default())
            .expect_err("inverted span must be rejected");
        assert!(
            matches!(
                err,
                SourceError::StaleIndex {
                    reason: StaleReason::SpanBeyondFile,
                    ..
                }
            ),
            "{err}"
        );
    }

    // ── CLI 参数面测试 ──

    #[test]
    fn cli_key_builder_enforces_mutual_exclusion() {
        let e = build_key_from_cli("", "", "", "").unwrap_err();
        assert!(e.to_string().contains("no locator key"), "{e}");
        assert!(
            e.to_string().contains("--qualified_name"),
            "usage text: {e}"
        );

        let e = build_key_from_cli("demo.f", "n1", "", "").unwrap_err();
        assert!(e.to_string().contains("multiple locator keys"), "{e}");

        let e = build_key_from_cli("", "", "/src/a.rs", "").unwrap_err();
        assert!(e.to_string().contains("成对给出"), "{e}");

        assert_eq!(
            build_key_from_cli("demo.f", "", "", "").unwrap(),
            SymbolKey::QualifiedName("demo.f".into())
        );
        assert_eq!(
            build_key_from_cli("", "n1", "", "").unwrap(),
            SymbolKey::NodeId("n1".into())
        );
        assert_eq!(
            build_key_from_cli("", "", "/src/a.rs", "f").unwrap(),
            SymbolKey::FilePathAndName {
                file_path: "/src/a.rs".into(),
                name: "f".into()
            }
        );
    }

    #[test]
    fn cli_key_builder_trims_whitespace_only_args() {
        assert_eq!(
            build_key_from_cli("  demo.f  ", "", "", "").unwrap(),
            SymbolKey::QualifiedName("demo.f".into())
        );
        assert!(
            build_key_from_cli("   ", "", "", "").is_err(),
            "whitespace-only counts as absent"
        );
    }

    #[test]
    fn cli_options_zero_means_default() {
        let o = cli_options(0, 0, 0);
        assert!(o.window.is_none());
        assert_eq!(o.max_lines.get(), 500);

        let o = cli_options(7, 9, 0);
        assert_eq!(
            o.window,
            Some(LineWindow {
                start: 7,
                end: Some(9)
            })
        );
        assert_eq!(o.max_lines.get(), 500);

        let o = cli_options(7, 0, 300);
        assert_eq!(
            o.window,
            Some(LineWindow {
                start: 7,
                end: None
            })
        );
        assert_eq!(o.max_lines.get(), 300);
    }

    // ── MCP 工具面测试 ──

    use crate::service::search::run_search;

    /// R-mcp-002 使用链：run_search 输出的 qualifiedName 直接作为
    /// run_read_symbol_source 的 QualifiedName 入参命中同一节点。
    /// （MCP wrapper 返回类型 = core 返回类型 `SymbolSource`，同构由类型系统
    /// 保证；参数映射经 `cli_options`/`build_key_from_cli` 测试覆盖。）
    #[test]
    fn search_qualified_name_feeds_source_read() {
        let (kit, _dir, _fp) = seed_fresh_workspace(SEED_SOURCE, 3);
        let output = run_search(&kit, "seed_fn", false, 10, "", "").expect("search finds seed_fn");
        assert!(output.count > 0, "search must hit seed_fn");
        let qn = output.results[0]["qualifiedName"]
            .as_str()
            .expect("search result carries qualifiedName")
            .to_string();

        let out = run_read_symbol_source(
            &kit,
            SymbolKey::QualifiedName(qn.clone()),
            &SourceOptions::default(),
        )
        .expect("source read via search's qualifiedName");
        assert_eq!(out.qualified_name, qn);
        assert_eq!(out.node_id, "fn_1");
        assert_eq!(out.source, SEED_SOURCE.trim_end_matches('\n'));
    }

    // ── 端到端测试：resolve → freshness → window → slice ──

    #[test]
    fn end_to_end_reads_exact_source_by_qualified_name() {
        let (kit, dir, file_path) = seed_fresh_workspace(SEED_SOURCE, 3);
        let out = run_read_symbol_source(
            &kit,
            SymbolKey::QualifiedName("demo.seed_fn".into()),
            &SourceOptions::default(),
        )
        .expect("end-to-end read");
        assert_eq!(out.source, SEED_SOURCE.trim_end_matches('\n'));
        assert_eq!(out.start_line, 1);
        assert_eq!(out.end_line, 3);
        assert_eq!(out.total_lines, 3);
        assert_eq!(out.label, "Function");
        assert_eq!(out.node_id, "fn_1");
        assert_eq!(out.file_path, file_path);
        let _ = dir;
    }

    #[test]
    fn end_to_end_windowed_read_slices_exact_lines() {
        let (kit, _dir, _fp) = seed_fresh_workspace(SEED_SOURCE, 3);
        let out = run_read_symbol_source(
            &kit,
            SymbolKey::QualifiedName("demo.seed_fn".into()),
            &SourceOptions {
                window: Some(LineWindow {
                    start: 2,
                    end: None,
                }),
                ..SourceOptions::default()
            },
        )
        .expect("windowed read");
        assert_eq!(out.source, "    body\n}".trim_end_matches('\n'));
        assert_eq!(out.start_line, 2);
        assert_eq!(out.end_line, 3);
        assert_eq!(out.total_lines, 3, "total_lines stays full span");
    }

    /// R-source-004：读盘路径只取 DB 节点行。构造 DB filePath 与入参
    /// file_path 故意不同的场景——source 必须来自 DB 记录路径。
    #[test]
    fn source_comes_from_db_recorded_path_not_input_key() {
        let (kit, _dir, _fp) = seed_fresh_workspace(SEED_SOURCE, 3);
        // 入参 file_path 故意指向别的文件名：查询键不命中则 NotFound；
        // 若实现误用入参路径读盘，也不会产出 DB 路径的内容。
        let out = run_read_symbol_source(
            &kit,
            SymbolKey::FilePathAndName {
                file_path: "/other/whatever.rs".into(),
                name: "seed_fn".into(),
            },
            &SourceOptions::default(),
        );
        assert!(
            matches!(out, Err(SourceError::SymbolNotFound { .. })),
            "input file_path is query-only: {out:?}"
        );

        // 正确键命中后，返回的 file_path 必须是 DB 记录的绝对路径
        let db_row_path = {
            let storage = storage_of(&kit);
            resolve_symbol(&*storage, &SymbolKey::QualifiedName("demo.seed_fn".into()))
                .expect("resolve")
                .file_path
        };
        let out = run_read_symbol_source(
            &kit,
            SymbolKey::NodeId("fn_1".into()),
            &SourceOptions::default(),
        )
        .expect("read via node id");
        assert_eq!(out.file_path, db_row_path);
        assert_eq!(out.source, SEED_SOURCE.trim_end_matches('\n'));
    }

    #[test]
    fn end_to_end_stale_file_refuses_to_return_source() {
        let (kit, dir, _fp) = seed_fresh_workspace(SEED_SOURCE, 3);
        std::fs::write(dir.path().join("a.rs"), "fn seed_fn() {\n    CHANGED\n}\n")
            .expect("modify file after index");
        let out = run_read_symbol_source(
            &kit,
            SymbolKey::QualifiedName("demo.seed_fn".into()),
            &SourceOptions::default(),
        );
        let err = out.expect_err("stale content must refuse");
        assert!(err.to_string().contains("codenexus index"), "{err}");
    }
}
