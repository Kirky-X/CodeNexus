// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! 图数据查询 — 统一经 [`QueryFacade`] 只读访问（不再直连 lbug）。

use std::path::Path;

use serde_json::Value as JsonValue;

use crate::i18n::t;
use crate::query::QueryFacade;
use crate::storage::schema::escape_cypher_string;

use super::{
    GraphData, GraphEdge, GraphNode, LabelCount, SchemaInfo, TracePath, TraceResult, TypeCount,
};

/// 可查询的节点类型（按优先级排序）— 与旧 graph-server 保持一致。
const QUERYABLE_LABELS: &[&str] = &[
    "Function",
    "Method",
    "Class",
    "Struct",
    "Enum",
    "Trait",
    "Interface",
    "Impl",
    "Constructor",
    "Variable",
    "GlobalVar",
    "Const",
    "Static",
    "Macro",
    "TypeAlias",
    "Namespace",
    "Module",
    "Test",
    "Handler",
    "Middleware",
    "Service",
    "Endpoint",
    "Route",
    "Event",
    "Property",
    "Field",
    "Record",
    "Template",
    "Union",
    "Variant",
    "Annotation",
    "Delegate",
    "Typedef",
    "Section",
    "Database",
    "Config",
];

/// 从行中按列名取字符串（`QueryFacade` 的行已是 JSON）。
fn get_str(columns: &[String], row: &[JsonValue], name: &str) -> Option<String> {
    let idx = columns.iter().position(|c| c == name)?;
    row.get(idx).and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// 从行中按列名取数值（`QueryFacade` 的行已是 JSON）。
fn get_f64(columns: &[String], row: &[JsonValue], name: &str) -> Option<f64> {
    let idx = columns.iter().position(|c| c == name)?;
    row.get(idx).and_then(|v| v.as_f64())
}

/// 查询 schema 统计信息。
pub(super) fn query_schema(db_path: &Path) -> Result<SchemaInfo, Box<dyn std::error::Error>> {
    let facade = QueryFacade::new_read_only(db_path)?;

    /* 节点标签统计 — 查询所有已知类型 */
    let mut node_labels = Vec::new();
    for label in QUERYABLE_LABELS {
        let cypher = format!("MATCH (n:{label}) RETURN count(*) AS cnt");
        if let Ok(result) = facade.cypher(&cypher) {
            if let Some(row) = result.rows.first() {
                let count = get_f64(&result.columns, row, "cnt").unwrap_or(0.0) as u64;
                if count > 0 {
                    node_labels.push(LabelCount {
                        label: label.to_string(),
                        count,
                    });
                }
            }
        }
    }
    node_labels.sort_by_key(|a| std::cmp::Reverse(a.count));

    /* 边总数 — 从 CodeRelation 表查询 */
    let result = facade.cypher("MATCH (r:CodeRelation) RETURN count(*) AS cnt")?;
    let total_edges: u64 = result
        .rows
        .first()
        .map(|row| get_f64(&result.columns, row, "cnt").unwrap_or(0.0) as u64)
        .unwrap_or(0);
    let edge_types = vec![TypeCount {
        r#type: "ALL".to_string(),
        count: total_edges,
    }];

    let total_nodes: u64 = node_labels.iter().map(|l| l.count).sum();

    Ok(SchemaInfo {
        node_labels,
        edge_types,
        total_nodes,
        total_edges,
    })
}

/// 球面半径。
const SPHERE_RADIUS: f64 = 400.0;

/// 3D 均匀球面坐标（Fibonacci sphere）— 所有节点精确分布在同一半径的球面上，
/// 视觉上像爆炸展开的球。
fn sphere_position(index: usize, total: usize) -> (f64, f64, f64) {
    let phi = (1.0_f64 - 2.0 * (index as f64 + 0.5) / total.max(1) as f64).acos();
    let theta = std::f64::consts::PI * (1.0 + 5.0_f64.sqrt()) * index as f64;
    (
        SPHERE_RADIUS * phi.sin() * theta.cos(),
        SPHERE_RADIUS * phi.sin() * theta.sin(),
        SPHERE_RADIUS * phi.cos(),
    )
}

/// 查询完成后对所有节点统一分配 Fibonacci sphere 坐标 — 保证同一球面均匀分布。
fn assign_sphere_positions(nodes: &mut [GraphNode]) {
    let total = nodes.len();
    for (i, node) in nodes.iter_mut().enumerate() {
        let (x, y, z) = sphere_position(i, total);
        node.x = x;
        node.y = y;
        node.z = z;
    }
}

/// 查询图数据（节点 + 边）。
pub(super) fn query_graph(
    db_path: &Path,
    project: &str,
    max_nodes: Option<u64>,
    _file_path: Option<&str>,
) -> Result<GraphData, Box<dyn std::error::Error>> {
    let facade = QueryFacade::new_read_only(db_path)?;
    let limit = max_nodes.unwrap_or(5000) as usize;

    /* 多类型节点查询 — 按优先级依次查询，合并结果 */
    let mut nodes = Vec::new();
    let mut name_to_id: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut node_counter = 0usize;

    /* 每个类型分配的配额 */
    let per_type_limit = (limit / 6).clamp(50, 500);

    for label in QUERYABLE_LABELS {
        if nodes.len() >= limit {
            break;
        }
        let type_limit = per_type_limit.min(limit - nodes.len());
        let cypher = format!(
            "MATCH (n:{label}) \
             RETURN n.name AS name, n.qualifiedName AS qualified_name, n.filePath AS file_path, \
                    n.project AS project, n.startLine AS start_line, n.endLine AS end_line, \
                    labels(n) AS labels \
             LIMIT {type_limit}"
        );

        let Ok(result) = facade.cypher(&cypher) else {
            continue; /* 该类型不存在或查询失败，跳过 */
        };

        if result.rows.is_empty() {
            continue;
        }

        for row in &result.rows {
            if nodes.len() >= limit {
                break;
            }
            let synth_id = format!("n{node_counter}");
            node_counter += 1;
            let name = get_str(&result.columns, row, "name").unwrap_or_default();

            let parsed_label = {
                let labels_val = get_str(&result.columns, row, "labels");
                if let Some(l) = labels_val {
                    if l.contains('[') {
                        l.trim_matches(|c| c == '[' || c == ']' || c == '"' || c == '\'')
                            .split(',')
                            .next()
                            .unwrap_or("Node")
                            .trim()
                            .to_string()
                    } else {
                        l
                    }
                } else {
                    (*label).to_string()
                }
            };

            if !name.is_empty() {
                name_to_id.insert(name.clone(), synth_id.clone());
            }
            /* 同时用 qualified_name 做映射 — 边的 source/target 通常使用 qualified_name */
            let qn = get_str(&result.columns, row, "qualified_name").unwrap_or_default();
            if !qn.is_empty() {
                name_to_id.insert(qn, synth_id.clone());
            }

            nodes.push(GraphNode {
                id: synth_id,
                label: parsed_label,
                name,
                file_path: get_str(&result.columns, row, "file_path"),
                project: get_str(&result.columns, row, "project")
                    .unwrap_or_else(|| project.to_string()),
                qualified_name: get_str(&result.columns, row, "qualified_name"),
                start_line: get_f64(&result.columns, row, "start_line").map(|v| v as u32),
                end_line: get_f64(&result.columns, row, "end_line").map(|v| v as u32),
                x: 0.0,
                y: 0.0,
                z: 0.0,
            });
        }
    }

    eprintln!(
        "[INFO] {}",
        t(
            "graph-node-query-info",
            &[
                ("types", QUERYABLE_LABELS.len().to_string()),
                ("nodes", nodes.len().to_string()),
            ]
        )
    );

    /* 查询边 — 至少一端匹配已加载节点，缺失端点自动补充为 stub 节点 */
    let edges = query_all_edges(&facade, limit as u64, &mut nodes, &mut name_to_id)?;

    /* 爆炸的球 — 全部节点（含 stub）统一分配同一球面的 Fibonacci sphere 坐标 */
    assign_sphere_positions(&mut nodes);

    let total_nodes = nodes.len() as u64;
    let total_edges = edges.len() as u64;

    Ok(GraphData {
        nodes,
        edges,
        total_nodes,
        total_edges,
    })
}

/// 查询所有边 — 至少一端在已加载节点中，缺失端点补充为 stub 节点。
fn query_all_edges(
    facade: &QueryFacade,
    limit: u64,
    nodes: &mut Vec<GraphNode>,
    name_to_id: &mut std::collections::HashMap<String, String>,
) -> Result<Vec<GraphEdge>, Box<dyn std::error::Error>> {
    /* 扫描更多边以提高命中率 */
    let scan_limit = limit.saturating_mul(200).clamp(10_000, 200_000);
    let cypher = format!(
        "MATCH (r:CodeRelation) RETURN r.source AS src_name, r.target AS tgt_name, r.type AS edge_type LIMIT {scan_limit}"
    );
    let result = facade.cypher(&cypher)?;
    let columns = result.columns;
    let rows = result.rows;
    let mut edges = Vec::new();
    let mut edge_counter = 0usize;
    let mut node_counter = nodes.len();
    for row in rows.iter() {
        if edges.len() >= limit as usize {
            break;
        }
        let src_name = get_str(&columns, row, "src_name").unwrap_or_default();
        let tgt_name = get_str(&columns, row, "tgt_name").unwrap_or_default();
        let edge_type =
            get_str(&columns, row, "edge_type").unwrap_or_else(|| "UNKNOWN".to_string());

        let src_id = name_to_id.get(&src_name).cloned();
        let tgt_id = name_to_id.get(&tgt_name).cloned();

        /* 至少一端必须匹配已加载节点 */
        if src_id.is_none() && tgt_id.is_none() {
            continue;
        }

        /* 为缺失端点创建 stub 节点 */
        let source = match src_id {
            Some(id) => id,
            None => {
                let synth_id = format!("n{node_counter}");
                node_counter += 1;
                name_to_id.insert(src_name.clone(), synth_id.clone());
                nodes.push(stub_node(&synth_id, &src_name));
                synth_id
            }
        };
        let target = match tgt_id {
            Some(id) => id,
            None => {
                let synth_id = format!("n{node_counter}");
                node_counter += 1;
                name_to_id.insert(tgt_name.clone(), synth_id.clone());
                nodes.push(stub_node(&synth_id, &tgt_name));
                synth_id
            }
        };

        edges.push(GraphEdge {
            id: format!("e{edge_counter}"),
            source,
            target,
            edge_type,
            confidence: 1.0,
            start_line: None,
            project: String::new(),
        });
        edge_counter += 1;
    }
    eprintln!(
        "[INFO] {}",
        t(
            "graph-edge-scan-info",
            &[
                ("rows", rows.len().to_string()),
                ("edges", edges.len().to_string()),
                ("total", nodes.len().to_string()),
            ]
        )
    );
    Ok(edges)
}

/// 边端点缺失时补充的 stub 节点。
fn stub_node(id: &str, name: &str) -> GraphNode {
    GraphNode {
        id: id.to_string(),
        label: "Function".into(),
        name: name.to_string(),
        file_path: None,
        project: String::new(),
        qualified_name: Some(name.to_string()),
        start_line: None,
        end_line: None,
        x: 0.0,
        y: 0.0,
        z: 0.0,
    }
}

/// 执行追踪查询 — 在 `CodeRelation` 单表 schema 上按方向逐跳展开。
///
/// 旧 graph-server 用关系模式 `(a)-[r:CALLS*1..N]->(b)` 直查；本仓 schema 把
/// 关系存放在 `CodeRelation` **节点表**（`source`/`target` 字符串列，见
/// `storage::schema` 的设计注记），关系类型模式在本引擎必然 Binder error，
/// 故改为逐跳展开：方向（downstream/upstream/both）、边类型集合与深度上限
/// 语义与旧实现一致；端点用 name 或 qualifiedName 解析（边表存限定名）。
/// 追踪每跳边查询的行数上限（防止单跳无界拉取）。
const TRACE_HOP_EDGE_LIMIT: u64 = 1_000;
/// 追踪每跳端点批量解析的行数上限。
const TRACE_HOP_NODE_LIMIT: u64 = 2_000;

pub(super) fn query_trace(
    db_path: &Path,
    project: &str,
    node_id: &str,
    mode: &str,
    direction: &str,
    max_depth: u32,
) -> Result<TraceResult, Box<dyn std::error::Error>> {
    let facade = QueryFacade::new_read_only(db_path)?;

    let edge_types: &[&str] = match mode {
        "call" => &["CALLS", "FFI_CALLS", "HTTP_CALLS", "ASYNC_CALLS"],
        "variable" => &["READS", "WRITES", "ACCESSES", "DATAFLOWS"],
        _ => &["CALLS", "READS", "WRITES", "ACCESSES"],
    };
    let dir_label = match direction {
        "downstream" => "downstream",
        "upstream" => "upstream",
        _ => "both",
    };
    let follow_out = direction != "upstream";
    let follow_in = direction != "downstream";

    let origin_node = query_single_node(&facade, node_id, project)?;

    let type_list = edge_types
        .iter()
        .map(|t| format!("'{t}'"))
        .collect::<Vec<_>>()
        .join(", ");

    let mut frontier: Vec<GraphNode> = vec![origin_node.clone()];
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    seen.insert(origin_node.name.clone());
    let mut targets: Vec<GraphNode> = Vec::new();

    /* 逐跳展开 — 每跳先按边表匹配前沿键收集原始端点名，再一次性批量解析节点
     * （早期逐端点各查一次库是 N+1：枢纽节点实测最坏 3.79s）。 */
    for _ in 0..max_depth {
        if frontier.is_empty() || targets.len() >= 100 {
            break;
        }
        let mut keys: Vec<String> = Vec::new();
        for node in &frontier {
            keys.push(node.name.clone());
            if let Some(qn) = &node.qualified_name {
                if !keys.contains(qn) {
                    keys.push(qn.clone());
                }
            }
        }
        let key_list = keys
            .iter()
            .map(|k| format!("'{}'", escape_cypher_string(k)))
            .collect::<Vec<_>>()
            .join(", ");

        /* 本跳的原始端点名（跨方向、跳内去重） */
        let mut raw_names: Vec<String> = Vec::new();
        let legs = [
            (follow_out, "source", "target"),
            (follow_in, "target", "source"),
        ];
        for (follow, near, far) in legs {
            if !follow {
                continue;
            }
            let cypher = format!(
                "MATCH (r:CodeRelation) WHERE r.{near} IN [{key_list}] AND r.type IN [{type_list}] \
                 RETURN DISTINCT r.{far} AS name LIMIT {TRACE_HOP_EDGE_LIMIT}"
            );
            let result = facade.cypher(&cypher)?;
            for row in &result.rows {
                if let Some(raw) = get_str(&result.columns, row, "name") {
                    if !raw_names.contains(&raw) {
                        raw_names.push(raw);
                    }
                }
            }
        }
        if raw_names.is_empty() {
            break;
        }

        /* 一次批量解析本跳全部端点 */
        let raw_list = raw_names
            .iter()
            .map(|k| format!("'{}'", escape_cypher_string(k)))
            .collect::<Vec<_>>()
            .join(", ");
        let cypher = format!(
            "MATCH (n) WHERE n.name IN [{raw_list}] OR n.qualifiedName IN [{raw_list}] \
             RETURN n.name AS name, n.qualifiedName AS qualified_name, n.filePath AS file_path, \
                    n.project AS project, n.startLine AS start_line, n.endLine AS end_line, \
                    labels(n) AS labels LIMIT {TRACE_HOP_NODE_LIMIT}"
        );
        let result = facade.cypher(&cypher)?;

        let mut next: Vec<GraphNode> = Vec::new();
        for row in &result.rows {
            let Some(name) = get_str(&result.columns, row, "name") else {
                continue;
            };
            if !seen.insert(name.clone()) {
                continue;
            }
            let node = node_from_row(&result.columns, row, name, project);
            targets.push(node.clone());
            next.push(node);
            if targets.len() >= 100 {
                break;
            }
        }
        frontier = next;
    }

    let paths = targets
        .into_iter()
        .map(|target_node| TracePath {
            nodes: vec![origin_node.clone(), target_node],
            edges: vec![],
        })
        .collect();

    Ok(TraceResult {
        origin: origin_node,
        paths,
        direction: dir_label.to_string(),
    })
}

/// 查询单个节点 — 按 `name` 或 `qualifiedName` 解析（边表存限定名）。
fn query_single_node(
    facade: &QueryFacade,
    node_name: &str,
    project: &str,
) -> Result<GraphNode, Box<dyn std::error::Error>> {
    let escaped = escape_cypher_string(node_name);
    let cypher = format!(
        "MATCH (n) WHERE n.name = '{escaped}' OR n.qualifiedName = '{escaped}' \
         RETURN n.name AS name, n.qualifiedName AS qualified_name, n.filePath AS file_path, \
                n.project AS project, n.startLine AS start_line, n.endLine AS end_line, \
                labels(n) AS labels \
         LIMIT 1",
    );

    let result = facade.cypher(&cypher)?;
    if let Some(row) = result.rows.first() {
        let name = get_str(&result.columns, row, "name").unwrap_or_else(|| node_name.to_string());
        Ok(node_from_row(&result.columns, row, name, project))
    } else {
        Err(crate::i18n::t("node-not-found", &[("name", node_name.to_string())]).into())
    }
}

/// 从查询行构造 [`GraphNode`]（字段映射与旧 graph-server 逐字一致）。
fn node_from_row(columns: &[String], row: &[JsonValue], name: String, project: &str) -> GraphNode {
    let label = {
        let labels_val = get_str(columns, row, "labels");
        if let Some(l) = labels_val {
            if l.contains('[') {
                l.trim_matches(|c| c == '[' || c == ']' || c == '"' || c == '\'')
                    .split(',')
                    .next()
                    .unwrap_or("Node")
                    .trim()
                    .to_string()
            } else {
                l
            }
        } else {
            "Node".to_string()
        }
    };
    GraphNode {
        id: name.clone(),
        label,
        name,
        file_path: get_str(columns, row, "file_path"),
        project: get_str(columns, row, "project").unwrap_or_else(|| project.to_string()),
        qualified_name: get_str(columns, row, "qualified_name"),
        start_line: get_f64(columns, row, "start_line").map(|v| v as u32),
        end_line: get_f64(columns, row, "end_line").map(|v| v as u32),
        x: 0.0,
        y: 0.0,
        z: 0.0,
    }
}

/// 快速统计数据库中的节点数和边数（用于项目列表）。
pub(super) fn quick_count(db_path: &Path) -> Result<(u64, u64), Box<dyn std::error::Error>> {
    let facade = QueryFacade::new_read_only(db_path)?;

    let node_count = {
        let result = facade.cypher("MATCH (n) RETURN count(n) AS cnt")?;
        result
            .rows
            .first()
            .and_then(|r| r.first())
            .and_then(json_count)
            .unwrap_or(0)
    };

    let edge_count = {
        let result = facade.cypher("MATCH (r:CodeRelation) RETURN count(*) AS cnt")?;
        result
            .rows
            .first()
            .and_then(|r| r.first())
            .and_then(json_count)
            .unwrap_or(0)
    };

    Ok((node_count, edge_count))
}

/// 把计数类 JSON 值解析为 u64（兼容 Number 与字符串两种形态）。
fn json_count(value: &JsonValue) -> Option<u64> {
    match value {
        JsonValue::Number(n) => n.as_u64(),
        JsonValue::String(s) => s.parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Language, Node, NodeLabel};
    use crate::storage::Repository;

    /// 建一个临时文件库并写入若干 Function 节点；返回（保持目录存活的 guard, 库路径）。
    fn temp_db_with_functions(names: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("demo.lbug");
        let repo = Repository::open(&db_path).expect("open");
        let nodes: Vec<Node> = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                Node::builder(NodeLabel::Function, *name, format!("demo.{name}"))
                    .id(&format!("f{i}"))
                    .project("demo")
                    .file_path("/src/main.rs")
                    .start_line(1)
                    .end_line(10)
                    .language(Language::Rust)
                    .signature("fn main()")
                    .build()
            })
            .collect();
        repo.save_nodes_stream(&nodes, NodeLabel::Function)
            .expect("save_nodes");
        drop(repo);
        (dir, db_path)
    }

    /// 建一个临时文件库：2 个 Function 节点 + 1 条 CALLS 边（alpha → beta）。
    fn temp_db_with_call_edge() -> (tempfile::TempDir, std::path::PathBuf) {
        use crate::model::{Edge, EdgeType};
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("demo.lbug");
        let repo = Repository::open(&db_path).expect("open");
        let nodes: Vec<Node> = ["alpha", "beta"]
            .iter()
            .enumerate()
            .map(|(i, name)| {
                Node::builder(NodeLabel::Function, *name, format!("demo.{name}"))
                    .id(&format!("f{i}"))
                    .project("demo")
                    .file_path("/src/main.rs")
                    .start_line(1)
                    .end_line(10)
                    .language(Language::Rust)
                    .signature("fn main()")
                    .build()
            })
            .collect();
        repo.save_nodes_stream(&nodes, NodeLabel::Function)
            .expect("save_nodes");
        let edge = Edge::new("demo.alpha", "demo.beta", EdgeType::Calls, "demo");
        repo.save_edges_stream(&[edge]).expect("save_edges");
        drop(repo);
        (dir, db_path)
    }

    #[test]
    fn graph_query_returns_nodes_edges_and_sphere_positions() {
        let (_dir, db_path) = temp_db_with_call_edge();
        let data = query_graph(&db_path, "demo", Some(100), None).expect("graph");
        assert_eq!(data.total_nodes, 2);
        assert_eq!(data.total_edges, 1);
        let edge = &data.edges[0];
        assert_eq!(edge.edge_type, "CALLS");
        // 两端都应映射到已加载节点（qualified_name 命中 name_to_id）
        let ids: Vec<&str> = data.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&edge.source.as_str()));
        assert!(ids.contains(&edge.target.as_str()));
        // Fibonacci 球面坐标已分配（非全零）
        assert!(data
            .nodes
            .iter()
            .all(|n| n.x != 0.0 || n.y != 0.0 || n.z != 0.0));
    }

    #[test]
    fn trace_downstream_follows_calls() {
        let (_dir, db_path) = temp_db_with_call_edge();
        let trace = query_trace(&db_path, "demo", "alpha", "call", "downstream", 3).expect("trace");
        assert_eq!(trace.direction, "downstream");
        assert_eq!(trace.origin.name, "alpha");
        assert_eq!(trace.paths.len(), 1);
        assert_eq!(trace.paths[0].nodes[1].name, "beta");
    }

    #[test]
    fn trace_missing_node_errors() {
        let (_dir, db_path) = temp_db_with_call_edge();
        assert!(query_trace(&db_path, "demo", "missing", "call", "downstream", 3).is_err());
    }

    #[test]
    fn queryable_labels_are_valid_node_labels() {
        use std::str::FromStr;
        for label in QUERYABLE_LABELS {
            assert!(
                crate::model::NodeLabel::from_str(&label.to_lowercase()).is_ok(),
                "QUERYABLE_LABELS 中的 {label} 不是有效的 NodeLabel"
            );
        }
    }

    /// 3 个函数指向同一枢纽 — 覆盖「跳内多端点一次批量解析」路径。
    #[test]
    fn trace_upstream_resolves_multiple_endpoints_in_one_hop() {
        use crate::model::{Edge, EdgeType};
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("demo.lbug");
        let repo = Repository::open(&db_path).expect("open");
        let names = ["hub", "a", "b", "c"];
        let nodes: Vec<Node> = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                Node::builder(NodeLabel::Function, *name, format!("demo.{name}"))
                    .id(&format!("f{i}"))
                    .project("demo")
                    .file_path("/src/main.rs")
                    .start_line(1)
                    .end_line(10)
                    .language(Language::Rust)
                    .signature("fn main()")
                    .build()
            })
            .collect();
        repo.save_nodes_stream(&nodes, NodeLabel::Function)
            .expect("save_nodes");
        let edges: Vec<Edge> = ["a", "b", "c"]
            .iter()
            .map(|s| Edge::new(format!("demo.{s}"), "demo.hub", EdgeType::Calls, "demo"))
            .collect();
        repo.save_edges_stream(&edges).expect("save_edges");
        drop(repo);

        let trace = query_trace(&db_path, "demo", "hub", "call", "upstream", 2).expect("trace");
        let mut got: Vec<String> = trace
            .paths
            .iter()
            .map(|p| p.nodes[1].name.clone())
            .collect();
        got.sort();
        assert_eq!(got, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[test]
    fn schema_counts_functions_and_zero_edges() {
        let (_dir, db_path) = temp_db_with_functions(&["alpha", "beta"]);
        let schema = query_schema(&db_path).expect("schema");
        assert_eq!(schema.total_nodes, 2);
        assert_eq!(schema.total_edges, 0);
        assert_eq!(schema.node_labels.len(), 1);
        assert_eq!(schema.node_labels[0].label, "Function");
        assert_eq!(schema.node_labels[0].count, 2);
        assert_eq!(schema.edge_types[0].r#type, "ALL");
        assert_eq!(schema.edge_types[0].count, 0);
    }

    #[test]
    fn quick_count_counts_all_tables_including_schema_meta() {
        let (_dir, db_path) = temp_db_with_functions(&["alpha", "beta", "gamma"]);
        // `MATCH (n)` 不带标签：在 3 个 Function 节点之外还会命中 SchemaMeta 单行表
        // （init_schema 盖章行）——与旧 graph-server 直连 lbug 时同一条查询的行为一致。
        assert_eq!(quick_count(&db_path).expect("quick_count"), (4, 0));
    }

    #[test]
    fn read_only_open_fails_on_missing_db() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("absent.lbug");
        assert!(query_schema(&missing).is_err());
        assert!(quick_count(&missing).is_err());
    }
}
