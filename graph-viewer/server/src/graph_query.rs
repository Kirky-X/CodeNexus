// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! 图数据查询模块 — 从 LadybugDB 读取节点和边数据

use std::path::Path;

use lbug::{Connection, Database, Value};
use serde_json::Value as JsonValue;

use super::{GraphData, GraphEdge, GraphNode, LabelCount, SchemaInfo, TracePath, TraceResult, TypeCount};

/// 创建只读数据库连接配置
fn read_only_config() -> lbug::SystemConfig {
    lbug::SystemConfig::default()
        .buffer_pool_size(256 * 1024 * 1024)
        .max_db_size(1024 * 1024 * 1024)
        .max_num_threads(4)
        .read_only(true)
}

/// 执行 Cypher 查询，返回 (列名列表, 行数据)
fn query_rows(
    conn: &Connection,
    cypher: &str,
) -> Result<(Vec<String>, Vec<Vec<JsonValue>>), Box<dyn std::error::Error>> {
    let mut result = conn.query(cypher)?;
    let columns = result.get_column_names();
    let mut rows = Vec::new();
    for row in &mut result {
        let json_row: Vec<JsonValue> = row.into_iter().map(value_to_json).collect();
        rows.push(json_row);
    }
    Ok((columns, rows))
}

/// 将 lbug::Value 转为 serde_json::Value
fn value_to_json(value: Value) -> JsonValue {
    match value {
        Value::Null(_) => JsonValue::Null,
        Value::Bool(b) => JsonValue::Bool(b),
        Value::Int8(i) => JsonValue::Number(i64::from(i).into()),
        Value::Int16(i) => JsonValue::Number(i64::from(i).into()),
        Value::Int32(i) => JsonValue::Number(i64::from(i).into()),
        Value::Int64(i) => JsonValue::Number(i.into()),
        Value::UInt8(u) => JsonValue::Number(i64::from(u).into()),
        Value::UInt16(u) => JsonValue::Number(i64::from(u).into()),
        Value::UInt32(u) => JsonValue::Number(i64::from(u).into()),
        Value::UInt64(u) => JsonValue::String(u.to_string()),
        Value::Int128(i) => JsonValue::String(i.to_string()),
        Value::Float(f) => {
            if let Some(n) = serde_json::Number::from_f64(f64::from(f)) {
                JsonValue::Number(n)
            } else {
                JsonValue::Null
            }
        }
        Value::Double(d) => {
            if let Some(n) = serde_json::Number::from_f64(d) {
                JsonValue::Number(n)
            } else {
                JsonValue::Null
            }
        }
        Value::String(s) => JsonValue::String(s),
        Value::Json(v) => v,
        Value::Blob(b) => JsonValue::String(String::from_utf8_lossy(&b).into_owned()),
        _ => JsonValue::String(format!("{:?}", value)),
    }
}

/// 从行中按列名取值
fn get_str(columns: &[String], row: &[JsonValue], name: &str) -> Option<String> {
    let idx = columns.iter().position(|c| c == name)?;
    row.get(idx).and_then(|v| v.as_str()).map(|s| s.to_string())
}

fn get_f64(columns: &[String], row: &[JsonValue], name: &str) -> Option<f64> {
    let idx = columns.iter().position(|c| c == name)?;
    row.get(idx).and_then(|v| v.as_f64())
}

/// 可查询的节点类型（按优先级排序）
const QUERYABLE_LABELS: &[&str] = &[
    "Function", "Method", "Class", "Struct", "Enum", "Trait", "Interface",
    "Impl", "Constructor", "Variable", "GlobalVar", "Const", "Static",
    "Macro", "TypeAlias", "Namespace", "Module", "Test", "Handler",
    "Middleware", "Service", "Endpoint", "Route", "Event", "Property",
    "Field", "Record", "Template", "Union", "Variant", "Annotation",
    "Delegate", "Typedef", "Section", "Database", "Config",
];

/// 球面半径
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

/// 查询完成后对所有节点统一分配 Fibonacci sphere 坐标 — 保证同一球面均匀分布
fn assign_sphere_positions(nodes: &mut [GraphNode]) {
    let total = nodes.len();
    for (i, node) in nodes.iter_mut().enumerate() {
        let (x, y, z) = sphere_position(i, total);
        node.x = x;
        node.y = y;
        node.z = z;
    }
}

/// 查询图数据（节点 + 边）
pub fn query_graph(
    db_path: &Path,
    project: &str,
    max_nodes: Option<u64>,
    _file_path: Option<&str>,
) -> Result<GraphData, Box<dyn std::error::Error>> {
    let db = Database::new(db_path, read_only_config())?;
    let conn = Connection::new(&db)?;
    let limit = max_nodes.unwrap_or(5000) as usize;

    /* 多类型节点查询 — 按优先级依次查询，合并结果 */
    let mut nodes = Vec::new();
    let mut name_to_id: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut node_counter = 0usize;

    /* 每个类型分配的配额 */
    let per_type_limit = (limit / 6).max(50).min(500);

    for label in QUERYABLE_LABELS {
        if nodes.len() >= limit {
            break;
        }
        let type_limit = per_type_limit.min(limit - nodes.len());
        let cypher = format!(
            "MATCH (n:{}) \
             RETURN n.name AS name, n.qualifiedName AS qualified_name, n.filePath AS file_path, \
                    n.project AS project, n.startLine AS start_line, n.endLine AS end_line, \
                    labels(n) AS labels \
             LIMIT {}",
            label, type_limit
        );

        let (cols, rows) = match query_rows(&conn, &cypher) {
            Ok(result) => result,
            Err(_) => continue, /* 该类型不存在或查询失败，跳过 */
        };

        if rows.is_empty() {
            continue;
        }


        for row in &rows {
            if nodes.len() >= limit {
                break;
            }
            let synth_id = format!("n{}", node_counter);
            node_counter += 1;
            let name = get_str(&cols, row, "name").unwrap_or_default();

            let parsed_label = {
                let labels_val = get_str(&cols, row, "labels");
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
            let qn = get_str(&cols, row, "qualified_name").unwrap_or_default();
            if !qn.is_empty() {
                name_to_id.insert(qn, synth_id.clone());
            }

            nodes.push(GraphNode {
                id: synth_id,
                label: parsed_label,
                name,
                file_path: get_str(&cols, row, "file_path"),
                project: get_str(&cols, row, "project").unwrap_or_else(|| project.to_string()),
                qualified_name: get_str(&cols, row, "qualified_name"),
                start_line: get_f64(&cols, row, "start_line").map(|v| v as u32),
                end_line: get_f64(&cols, row, "end_line").map(|v| v as u32),
                x: 0.0, y: 0.0, z: 0.0,
            });
        }
    }

    eprintln!("[INFO] Queried {} node types, got {} nodes", QUERYABLE_LABELS.len(), nodes.len());

    /* 查询边 — 至少一端匹配已加载节点，缺失端点自动补充为 stub 节点 */
    let edges = query_all_edges(&conn, limit as u64, &mut nodes, &mut name_to_id)?;

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

/// 查询所有边 — 至少一端在已加载节点中，缺失端点补充为 stub 节点
fn query_all_edges(
    conn: &Connection,
    limit: u64,
    nodes: &mut Vec<GraphNode>,
    name_to_id: &mut std::collections::HashMap<String, String>,
) -> Result<Vec<GraphEdge>, Box<dyn std::error::Error>> {
    /* 扫描更多边以提高命中率 */
    let scan_limit = (limit * 200).max(10_000).min(200_000);
    let cypher = format!(
        "MATCH (r:CodeRelation) RETURN r.source AS src_name, r.target AS tgt_name, r.type AS edge_type LIMIT {}",
        scan_limit
    );
    let (cols, rows) = query_rows(conn, &cypher)?;
    let mut edges = Vec::new();
    let mut edge_counter = 0usize;
    let mut node_counter = nodes.len();
    for row in rows.iter() {
        if edges.len() >= limit as usize {
            break;
        }
        let src_name = get_str(&cols, row, "src_name").unwrap_or_default();
        let tgt_name = get_str(&cols, row, "tgt_name").unwrap_or_default();
        let edge_type = get_str(&cols, row, "edge_type").unwrap_or_else(|| "UNKNOWN".to_string());

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
                let synth_id = format!("n{}", node_counter);
                node_counter += 1;
                name_to_id.insert(src_name.clone(), synth_id.clone());
                nodes.push(GraphNode {
                    id: synth_id.clone(),
                    label: "Function".into(),
                    name: src_name.clone(),
                    file_path: None,
                    project: String::new(),
                    qualified_name: Some(src_name),
                    start_line: None,
                    end_line: None,
                    x: 0.0, y: 0.0, z: 0.0,
                });
                synth_id
            }
        };
        let target = match tgt_id {
            Some(id) => id,
            None => {
                let synth_id = format!("n{}", node_counter);
                node_counter += 1;
                name_to_id.insert(tgt_name.clone(), synth_id.clone());
                nodes.push(GraphNode {
                    id: synth_id.clone(),
                    label: "Function".into(),
                    name: tgt_name.clone(),
                    file_path: None,
                    project: String::new(),
                    qualified_name: Some(tgt_name),
                    start_line: None,
                    end_line: None,
                    x: 0.0, y: 0.0, z: 0.0,
                });
                synth_id
            }
        };

        edges.push(GraphEdge {
            id: format!("e{}", edge_counter),
            source,
            target,
            edge_type,
            confidence: 1.0,
            start_line: None,
            project: String::new(),
        });
        edge_counter += 1;
    }
    eprintln!("[INFO] Edge scan: {} rows, {} edges matched, {} total nodes", rows.len(), edges.len(), nodes.len());
    Ok(edges)
}


/// 查询 schema 统计信息
pub fn query_schema(db_path: &Path) -> Result<SchemaInfo, Box<dyn std::error::Error>> {
    let db = Database::new(db_path, read_only_config())?;
    let conn = Connection::new(&db)?;

    /* 节点标签统计 — 查询所有已知类型 */
    let mut node_labels = Vec::new();
    for label in QUERYABLE_LABELS {
        let cypher = format!("MATCH (n:{}) RETURN count(*) AS cnt", label);
        if let Ok((cols, rows)) = query_rows(&conn, &cypher) {
            if let Some(row) = rows.first() {
                let count = get_f64(&cols, row, "cnt").unwrap_or(0.0) as u64;
                if count > 0 {
                    node_labels.push(LabelCount { label: label.to_string(), count });
                }
            }
        }
    }
    node_labels.sort_by(|a, b| b.count.cmp(&a.count));

    /* 边总数 — 从 CodeRelation 表查询 */
    let (cols, rows) = query_rows(
        &conn,
        "MATCH (r:CodeRelation) RETURN count(*) AS cnt",
    )?;
    let total_edges: u64 = rows.first().map(|row| {
        get_f64(&cols, row, "cnt").unwrap_or(0.0) as u64
    }).unwrap_or(0);
    let mut edge_types = Vec::new();
    edge_types.push(TypeCount { r#type: "ALL".to_string(), count: total_edges });

    let total_nodes: u64 = node_labels.iter().map(|l| l.count).sum();

    Ok(SchemaInfo {
        node_labels,
        edge_types,
        total_nodes,
        total_edges,
    })
}

/// 执行追踪查询 — 使用 name 属性而非 id()
pub fn query_trace(
    db_path: &Path,
    project: &str,
    node_id: &str,
    mode: &str,
    direction: &str,
    max_depth: u32,
) -> Result<TraceResult, Box<dyn std::error::Error>> {
    let db = Database::new(db_path, read_only_config())?;
    let conn = Connection::new(&db)?;

    let edge_types = match mode {
        "call" => vec!["CALLS", "FFI_CALLS", "HTTP_CALLS", "ASYNC_CALLS"],
        "variable" => vec!["READS", "WRITES", "ACCESSES", "DATAFLOWS"],
        _ => vec!["CALLS", "READS", "WRITES", "ACCESSES"],
    };

    let pattern = match direction {
        "downstream" => format!("(a)-[r:{}*1..{}]->(b)", edge_types.join("|"), max_depth),
        "upstream" => format!("(a)<-[r:{}*1..{}]-(b)", edge_types.join("|"), max_depth),
        _ => format!("(a)-[r:{}*1..{}]-(b)", edge_types.join("|"), max_depth),
    };
    let dir_label = match direction {
        "downstream" => "downstream",
        "upstream" => "upstream",
        _ => "both",
    };

    /* 使用 name 属性查找节点 */
    let cypher = format!(
        "MATCH {} WHERE a.name = '{}' AND a.project = '{}' \
         RETURN DISTINCT b.name AS target_name LIMIT 100",
        pattern,
        escape_cypher_str(node_id),
        escape_cypher_str(project),
    );

    let (cols, rows) = query_rows(&conn, &cypher)?;
    let target_names: Vec<String> = rows
        .iter()
        .filter_map(|row| get_str(&cols, row, "target_name"))
        .collect();

    let origin_node = query_single_node(&conn, node_id, project)?;

    let mut paths = Vec::new();
    for target_name in &target_names {
        if let Ok(target_node) = query_single_node(&conn, target_name, project) {
            paths.push(TracePath {
                nodes: vec![origin_node.clone(), target_node],
                edges: vec![],
            });
        }
    }

    Ok(TraceResult {
        origin: origin_node,
        paths,
        direction: dir_label.to_string(),
    })
}

/// 查询单个节点 — 使用 name 属性查找
fn query_single_node(
    conn: &Connection,
    node_name: &str,
    project: &str,
) -> Result<GraphNode, Box<dyn std::error::Error>> {
    let cypher = format!(
        "MATCH (n) WHERE n.name = '{}' \
         RETURN n.name AS name, n.qualifiedName AS qualified_name, n.filePath AS file_path, \
                n.project AS project, n.startLine AS start_line, n.endLine AS end_line, \
                labels(n) AS labels \
         LIMIT 1",
        escape_cypher_str(node_name),
    );

    let (cols, rows) = query_rows(conn, &cypher)?;
    if let Some(row) = rows.first() {
        let name = get_str(&cols, row, "name").unwrap_or_else(|| node_name.to_string());
        let label = {
            let labels_val = get_str(&cols, row, "labels");
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
        Ok(GraphNode {
            id: name.clone(),
            label,
            name,
            file_path: get_str(&cols, row, "file_path"),
            project: get_str(&cols, row, "project").unwrap_or_else(|| project.to_string()),
            qualified_name: get_str(&cols, row, "qualified_name"),
            start_line: get_f64(&cols, row, "start_line").map(|v| v as u32),
            end_line: get_f64(&cols, row, "end_line").map(|v| v as u32),
            x: 0.0,
            y: 0.0,
            z: 0.0,
        })
    } else {
        Err(format!("节点 '{}' 未找到", node_name).into())
    }
}

/// 转义 Cypher 字符串中的特殊字符
fn escape_cypher_str(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// 快速统计数据库中的节点数和边数（用于项目列表）
pub fn quick_count(db_path: &Path) -> Result<(u64, u64), Box<dyn std::error::Error>> {
    let db = Database::new(db_path, read_only_config())?;
    let conn = Connection::new(&db)?;

    let node_count = {
        let (_, rows) = query_rows(&conn, "MATCH (n) RETURN count(n) AS cnt")?;
        rows.first()
            .and_then(|r| r.first())
            .and_then(|v| match v {
                JsonValue::Number(n) => n.as_u64(),
                JsonValue::String(s) => s.parse().ok(),
                _ => None,
            })
            .unwrap_or(0)
    };

    let edge_count = {
        let (_, rows) = query_rows(&conn, "MATCH (r:CodeRelation) RETURN count(*) AS cnt")?;
        rows.first()
            .and_then(|r| r.first())
            .and_then(|v| match v {
                JsonValue::Number(n) => n.as_u64(),
                JsonValue::String(s) => s.parse().ok(),
                _ => None,
            })
            .unwrap_or(0)
    };

    Ok((node_count, edge_count))
}
