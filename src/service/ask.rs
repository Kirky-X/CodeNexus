// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

use std::fmt;

use serde::Serialize;
use serde_json::Value;

use crate::kit::{AsyncKit, AsyncReady};
use crate::service::detect_changes::run_detect_changes;
use crate::service::error::CodeNexusError;
#[cfg(feature = "cli")]
use crate::service::error::{kit_not_initialized, wrap_error};
use crate::service::impact::run_impact;
#[cfg(feature = "cli")]
use crate::service::runtime::kit;
use crate::service::search::run_search;
use crate::service::status::run_status;
use crate::service::trace::run_trace;

#[cfg(feature = "cli")]
use sdforge::forge;
#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;

#[cfg(feature = "complexity")]
use crate::analysis::complexity::ComplexityAnalyzer;
#[cfg(feature = "analysis")]
use crate::service::architecture::run_architecture;
use crate::service::context::run_context;
#[cfg(feature = "analysis")]
use crate::service::dead_code::{run_dead_code, DeadCodeParams};
#[cfg(feature = "complexity")]
use crate::service::project::resolve_project_id;
use crate::service::query::run_query;

/// One routing rule: keywords → command core.
struct IntentRule {
    /// Lowercase keywords; any hit activates the rule.
    keywords: &'static [&'static str],
    command: &'static str,
    /// Whether the rule consumes the extracted symbol.
    needs_symbol: bool,
    /// Equivalent CLI suggestion template (`{symbol}` substituted).
    suggestion: &'static str,
}

/// The static routing table. Order matters only for equal-confidence
/// display; selection sorts by matched-keyword length (specificity).
static RULES: &[IntentRule] = &[
    IntentRule {
        keywords: &["谁调用", "调用者", "被谁调用", "callers", "who calls", "call chain", "调用链"],
        command: "trace",
        needs_symbol: true,
        suggestion: "codenexus trace --symbol {symbol} --trace_type calls --depth 3",
    },
    IntentRule {
        keywords: &["影响", "impact", "波及", "blast radius", "改动会影响", "breaking"],
        command: "impact",
        needs_symbol: true,
        suggestion: "codenexus impact --symbol {symbol} --depth 3",
    },
    IntentRule {
        keywords: &["上下文", "360", "context", "symbol view", "全景"],
        command: "context",
        needs_symbol: true,
        suggestion: "codenexus context --symbol {symbol} --depth 1",
    },
    IntentRule {
        keywords: &["死代码", "dead code", "未使用", "unused", "没人用", "没被使用"],
        command: "dead_code",
        needs_symbol: false,
        suggestion: "codenexus dead_code --project {project}",
    },
    IntentRule {
        keywords: &["复杂度", "complexity", "圈复杂度", "cognitive", "可维护性"],
        command: "complexity",
        needs_symbol: false,
        suggestion: "codenexus complexity --project {project}",
    },
    IntentRule {
        keywords: &["架构", "architecture", "模块结构", "分层", "结构概览"],
        command: "architecture",
        needs_symbol: false,
        suggestion: "codenexus architecture --project {project}",
    },
    IntentRule {
        keywords: &["搜索", "search", "查找", "找一下", "find", "在哪定义"],
        command: "search",
        needs_symbol: true,
        suggestion: "codenexus search --text {symbol} --limit 20",
    },
    IntentRule {
        keywords: &["变更", "未提交", "what changed", "detect changes", "改了什么", "diff 风险"],
        command: "detect_changes",
        needs_symbol: false,
        suggestion: "codenexus detect_changes --path . --mode unstaged",
    },
    IntentRule {
        keywords: &["状态", "status", "索引状态", "索引了哪些", "哪些项目"],
        command: "status",
        needs_symbol: false,
        suggestion: "codenexus status",
    },
    IntentRule {
        keywords: &["query", "cypher", "图里长什么样", "图查询"],
        command: "query",
        needs_symbol: true,
        suggestion: "codenexus query --cypher \"MATCH (n:Function) WHERE n.name = '{symbol}' RETURN n LIMIT 20\"",
    },
];

/// A routed intent candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct IntentCandidate {
    pub command: &'static str,
    /// Extracted symbol (backtick/quote-wrapped first, then identifier-like).
    pub symbol: Option<String>,
    pub confidence: f32,
    /// The keywords that activated this rule.
    pub matched_keywords: Vec<&'static str>,
}

impl fmt::Display for IntentCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (confidence {:.2})", self.command, self.confidence)
    }
}

/// Extracts a symbol from the question: backtick/quoted spans first, then
/// identifier-like tokens (`camelCase`/`snake_case`/`a::b` paths), longest first.
pub fn extract_symbol(question: &str) -> Option<String> {
    // 1) Backtick / quote wrapped spans.
    for (open, close) in [('`', '`'), ('"', '"'), ('\'', '\''), ('“', '”')] {
        if let Some(start) = question.find(open) {
            if let Some(len) = question[start + open.len_utf8()..].find(close) {
                let candidate = &question[start + open.len_utf8()..start + open.len_utf8() + len];
                if !candidate.trim().is_empty() {
                    return Some(candidate.trim().to_string());
                }
            }
        }
    }
    // 2) Identifier-like tokens: contain '_' or an uppercase letter inside,
    //    or a path qualifier `::`; at least 3 chars; must start with an
    //    ASCII identifier char (CJK tokens are natural language, not symbols).
    let mut best: Option<&str> = None;
    for token in question.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':')) {
        let has_path = token.contains("::");
        let has_underscore = token.contains('_');
        let has_inner_upper = token.chars().skip(1).any(char::is_uppercase);
        if token.len() >= 3
            && !token.ends_with(':')
            && (has_path || has_underscore || has_inner_upper)
            && best.is_none_or(|b| token.len() > b.len())
        {
            best = Some(token);
        }
    }
    best.map(str::to_string)
}

/// Routes a question to ranked intent candidates (best first).
pub fn route(question: &str) -> Vec<IntentCandidate> {
    let lower = question.to_lowercase();
    let symbol = extract_symbol(question);
    let mut candidates: Vec<IntentCandidate> = Vec::new();
    for rule in RULES {
        let matched: Vec<&'static str> = rule
            .keywords
            .iter()
            .copied()
            .filter(|kw| lower.contains(kw))
            .collect();
        if matched.is_empty() {
            continue;
        }
        let confidence = match (rule.needs_symbol, &symbol) {
            (true, Some(_)) => 0.9,
            (true, None) => 0.5,
            (false, _) => 0.7,
        };
        candidates.push(IntentCandidate {
            command: rule.command,
            symbol: symbol.clone(),
            confidence,
            matched_keywords: matched,
        });
    }
    candidates.sort_by(|a, b| {
        b.confidence
            .total_cmp(&a.confidence)
            .then_with(|| {
                let la = a
                    .matched_keywords
                    .iter()
                    .map(|kw| kw.len())
                    .max()
                    .unwrap_or(0);
                let lb = b
                    .matched_keywords
                    .iter()
                    .map(|kw| kw.len())
                    .max()
                    .unwrap_or(0);
                lb.cmp(&la)
            })
            .then_with(|| a.command.cmp(b.command))
    });
    candidates
}

/// JSON-serializable ask output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AskOutput {
    pub question: String,
    /// The routed intent (`None` when nothing matched).
    pub intent: Option<IntentDescriptor>,
    /// Whether the intent was executed.
    pub executed: bool,
    /// Target command's JSON result (when executed successfully).
    pub result: Option<Value>,
    /// Equivalent CLI command strings (degraded paths and no-match help).
    pub suggestions: Vec<String>,
}

/// Serializable view of the chosen intent.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct IntentDescriptor {
    pub command: String,
    pub symbol: Option<String>,
    pub confidence: f32,
    pub matched_keywords: Vec<String>,
}

/// Renders the equivalent CLI suggestion for a candidate.
fn suggestion_for(candidate: &IntentCandidate, project: &str) -> String {
    let rule = RULES
        .iter()
        .find(|r| r.command == candidate.command)
        .expect("rule exists for every routed command");
    let project = if project.trim().is_empty() {
        "<project>"
    } else {
        project
    };
    rule.suggestion
        .replace(
            "{symbol}",
            candidate.symbol.as_deref().unwrap_or("<symbol>"),
        )
        .replace("{project}", project)
}

/// Escapes the symbol for the generated name-lookup Cypher.
fn cypher_name_lookup(symbol: &str) -> String {
    let escaped = crate::storage::schema::escape_cypher_string(symbol);
    format!("MATCH (n:Function) WHERE n.name = '{escaped}' RETURN n LIMIT 20")
}

/// Executes the routed intent in-process and returns the target command's
/// JSON value. `project` applies to project-scoped commands (dead_code,
/// complexity, architecture).
fn execute_intent(
    kit: &AsyncKit<AsyncReady>,
    candidate: &IntentCandidate,
    question: &str,
    project: &str,
) -> Result<Value, CodeNexusError> {
    let value = match candidate.command {
        "trace" => {
            let sym = candidate.symbol.as_deref().ok_or_else(|| {
                CodeNexusError::InvalidInput("trace intent requires a symbol".to_string())
            })?;
            serde_json::to_value(run_trace(kit, sym, "calls", 3, "", false, false)?)?
        }
        "impact" => {
            let sym = candidate.symbol.as_deref().ok_or_else(|| {
                CodeNexusError::InvalidInput("impact intent requires a symbol".to_string())
            })?;
            serde_json::to_value(run_impact(kit, sym, 3, "", 0, false)?)?
        }
        "context" => {
            let sym = candidate.symbol.as_deref().ok_or_else(|| {
                CodeNexusError::InvalidInput("context intent requires a symbol".to_string())
            })?;
            serde_json::to_value(run_context(kit, sym, 1)?)?
        }
        "search" => {
            let text = candidate
                .symbol
                .clone()
                .unwrap_or_else(|| question.to_string());
            serde_json::to_value(run_search(kit, &text, false, 20, "", project)?)?
        }
        // dead_code/architecture 服务为 analysis 门控（api-review/diagram 等隐含 analysis）
        #[cfg(feature = "analysis")]
        "dead_code" => serde_json::to_value(run_dead_code(
            kit,
            &DeadCodeParams {
                project: project.to_string(),
                ..DeadCodeParams::default()
            },
        )?)?,
        #[cfg(feature = "analysis")]
        "architecture" => serde_json::to_value(run_architecture(kit, project)?)?,
        "status" => serde_json::to_value(run_status(kit)?)?,
        "detect_changes" => serde_json::to_value(run_detect_changes(kit, ".", "unstaged")?)?,
        "query" => {
            let sym = candidate.symbol.as_deref().ok_or_else(|| {
                CodeNexusError::InvalidInput("query intent requires a symbol".to_string())
            })?;
            serde_json::to_value(run_query(kit, &cypher_name_lookup(sym))?)?
        }
        #[cfg(feature = "complexity")]
        "complexity" => {
            let storage = kit.require::<crate::kit::StorageModule>()?;
            let project_id = resolve_project_id(&*storage, project)?;
            let analyzer = ComplexityAnalyzer::new(&*storage);
            let entries = analyzer
                .analyze(&project_id)
                .map_err(CodeNexusError::from)?;
            let mut green = 0usize;
            let mut yellow = 0usize;
            let mut red = 0usize;
            let mut critical = 0usize;
            for e in &entries {
                match e.overall_severity {
                    crate::analysis::complexity::Severity::Green => green += 1,
                    crate::analysis::complexity::Severity::Yellow => yellow += 1,
                    crate::analysis::complexity::Severity::Red => red += 1,
                    crate::analysis::complexity::Severity::Critical => critical += 1,
                }
            }
            serde_json::json!({
                "project": project,
                "total": entries.len(),
                "green": green,
                "yellow": yellow,
                "red": red,
                "critical": critical,
            })
        }
        #[cfg(not(feature = "complexity"))]
        "complexity" => {
            return Err(CodeNexusError::InvalidInput(
                "complexity intent requires the `complexity` feature".to_string(),
            ))
        }
        other => {
            return Err(CodeNexusError::InvalidInput(format!(
                "no executor for routed command '{other}'"
            )))
        }
    };
    Ok(value)
}

/// Core logic — routes and (optionally) executes against an injected Kit.
pub fn run_ask(
    kit: &AsyncKit<AsyncReady>,
    question: &str,
    execute: bool,
    project: &str,
) -> AskOutput {
    let candidates = route(question);
    let best = candidates.first().cloned();
    let mut suggestions: Vec<String> = Vec::new();
    for candidate in candidates.iter().take(3) {
        suggestions.push(suggestion_for(candidate, project));
    }

    let intent = best.as_ref().map(|c| IntentDescriptor {
        command: c.command.to_string(),
        symbol: c.symbol.clone(),
        confidence: c.confidence,
        matched_keywords: c.matched_keywords.iter().map(ToString::to_string).collect(),
    });

    let executable = best
        .as_ref()
        .is_some_and(|c| execute && c.confidence >= 0.6);
    let (executed, result) = match (executable, best) {
        (true, Some(candidate)) => match execute_intent(kit, &candidate, question, project) {
            Ok(value) => (true, Some(value)),
            // Degrade: keep exit 0, surface the equivalent CLI instead.
            Err(_err) => (true, None),
        },
        _ => (false, None),
    };

    AskOutput {
        question: question.to_string(),
        intent,
        executed,
        result,
        suggestions,
    }
}

/// CLI wrapper — prints the ask receipt to stdout as JSON (always exit 0 on
/// routing success; execution failures degrade to suggestions).
#[cfg(feature = "cli")]
#[forge(
    name = "ask",
    version = "0.4.0",
    description = "Natural-language entry point: routes a question to an existing command core (trace/impact/context/search/dead_code/complexity/architecture/status/detect_changes/query) and executes it in-process. Low-confidence routes return suggestions instead of executing. Params: question (required); execute — run the matched command (default true); project — project name or id for project-scoped commands.",
    cli = true
)]
async fn ask(question: String, execute: bool, project: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let output = run_ask(&kit, &question, execute, &project);
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
#[cfg(any(feature = "cli", feature = "mcp"))]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageModule};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_ask_testdb");
        (dir, path)
    }

    fn build_kit_for_db(db: &std::path::Path) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    // --- symbol extraction ---

    #[test]
    fn extracts_backtick_quoted_symbol_first() {
        assert_eq!(
            extract_symbol("谁调用了 `parse_source` 这个函数？"),
            Some("parse_source".to_string())
        );
        assert_eq!(
            extract_symbol("impact of \"run_query\" on the repo"),
            Some("run_query".to_string())
        );
    }

    #[test]
    fn extracts_identifier_like_token_as_fallback() {
        assert_eq!(
            extract_symbol("trace the parse_source function"),
            Some("parse_source".to_string())
        );
        assert_eq!(
            extract_symbol("what calls StorageModule::query here"),
            Some("StorageModule::query".to_string())
        );
    }

    #[test]
    fn no_symbol_in_plain_question() {
        assert_eq!(extract_symbol("复杂度怎么样"), None);
    }

    // --- routing ---

    #[test]
    fn routes_callers_question_to_trace_with_symbol() {
        let candidates = route("谁调用了 `parse_source`");
        assert!(!candidates.is_empty());
        let best = &candidates[0];
        assert_eq!(best.command, "trace");
        assert_eq!(best.symbol.as_deref(), Some("parse_source"));
        assert!(
            best.confidence >= 0.6,
            "keyword+symbol must be executable: {}",
            best.confidence
        );
        assert!(best.matched_keywords.contains(&"谁调用"));
    }

    #[test]
    fn routes_impact_question() {
        let candidates = route("`foo` 的影响面是什么");
        assert_eq!(candidates[0].command, "impact");
        assert_eq!(candidates[0].symbol.as_deref(), Some("foo"));
    }

    #[test]
    fn symbol_free_rules_route_without_symbol() {
        let candidates = route("项目里有哪些死代码");
        assert_eq!(candidates[0].command, "dead_code");
        assert_eq!(candidates[0].confidence, 0.7);
        let complexity = route("复杂度怎么样");
        assert_eq!(complexity[0].command, "complexity");
    }

    #[test]
    fn unmatched_question_has_no_candidates() {
        assert!(route("今天天气怎么样").is_empty());
    }

    #[test]
    fn symbol_rule_without_symbol_stays_below_threshold() {
        let candidates = route("谁调用它");
        let trace = candidates
            .iter()
            .find(|c| c.command == "trace")
            .expect("trace candidate");
        assert!(trace.confidence < 0.6, "no symbol → not executable");
    }

    // --- execution (kit-based) ---

    fn seed_symbol(kit: &AsyncKit<AsyncReady>) {
        let storage = kit.require::<StorageModule>().unwrap();
        storage
            .execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'seed_fn', qualifiedName: 'demo.seed_fn', filePath: '/src/s.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});")
            .unwrap();
        storage
            .execute("CREATE (:Function {id: 'f2', project: 'demo', name: 'seed_caller', qualifiedName: 'demo.seed_caller', filePath: '/src/c.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});")
            .unwrap();
        storage
            .execute("CREATE (:CodeRelation {id: 'e1', source: 'f2', target: 'f1', type: 'CALLS', confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 2, project: 'demo'});")
            .unwrap();
    }

    #[test]
    fn run_ask_executes_trace_intent() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        seed_symbol(&kit);
        let out = run_ask(&kit, "谁调用了 `seed_fn`", true, "");
        assert_eq!(out.intent.as_ref().unwrap().command, "trace");
        assert!(out.executed);
        let result = out.result.expect("trace result present");
        assert!(result.is_object(), "trace result must be a JSON object");
    }

    #[test]
    fn run_ask_execute_false_skips_execution() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        seed_symbol(&kit);
        let out = run_ask(&kit, "谁调用了 `seed_fn`", false, "");
        assert!(!out.executed);
        assert!(out.result.is_none());
        assert!(out.intent.is_some());
    }

    #[test]
    fn run_ask_degrades_to_suggestions_on_unknown_symbol() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let out = run_ask(&kit, "谁调用了 `missing_symbol`", true, "");
        assert!(out.executed);
        assert!(out.result.is_none(), "failed execution → no result");
        assert!(
            !out.suggestions.is_empty(),
            "suggestions must carry the equivalent CLI"
        );
        assert!(out.suggestions[0].contains("trace --symbol missing_symbol"));
    }

    #[test]
    fn run_ask_without_match_lists_no_intent() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let out = run_ask(&kit, "今天天气怎么样", true, "");
        assert!(out.intent.is_none());
        assert!(!out.executed);
    }

    #[test]
    fn run_ask_status_intent_executes_on_empty_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let out = run_ask(&kit, "当前的索引状态如何", true, "");
        assert_eq!(out.intent.as_ref().unwrap().command, "status");
        assert!(out.executed);
        assert!(out.result.is_some(), "status succeeds even on empty DB");
    }
}
