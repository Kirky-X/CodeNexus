// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Cross-validation verify harness — folded into the main CLI.
//!
//! 原 `codenexus-verify` 独立二进制（`[[bin]]` 自定义 path 违规形态）的
//! 实现整体迁入库侧（`orchestrator` + `codenexus_stats` + `gitnexus_client`
//! + `query_compare` + `report` + `type_map`），入口折叠为主 CLI 的
//! `verify` 子命令：`codenexus verify <single|batch|fetch-samples>`。
//! 功能面与产物路径（`tools/verification/results/`、`samples.json`、
//! `type_map.json`、`queries/*.cql`）不变——数据文件仍留在
//! `tools/verification/`，只有 Rust 入口迁入。
//!
//! `verify` 不使用 Kit：single/batch 通过子进程 `cargo run --bin codenexus
//! -- index`（release）自索引，统计与查询经 `storage::Repository` 直开
//! `./codenexus.lbug`——主入口为它跳过 Kit 构建（同 `webui` 口径）。

pub mod codenexus_stats;
pub mod gitnexus_client;
pub(crate) mod orchestrator;
pub mod query_compare;
pub mod report;
pub mod type_map;

pub(crate) use orchestrator::{run_batch, run_fetch_samples, run_single_orchestrator, DEFAULT_DB};

use std::path::{Path, PathBuf};

#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::error::{to_api_error, wrap_error, CodeNexusError};
use sdforge::forge;
use sdforge::prelude::ApiError;

/// 解析 `--only` 过滤串（原独立入口的 repeatable `--only`）：逗号分隔，
/// 空串 = 不过滤。空白项剔除，保持与逐项过滤等价。
fn parse_only_filter(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// CLI wrapper — cross-validation harness 主入口（verify 子命令）。
///
/// `action` 选 single|batch|fetch-samples；其余参数按 action 语义消费
/// （未用到的参数忽略，与原独立入口的子命令隔离一致）。
// 8 参数是原独立入口三个子命令的平铺等价面（forge 宏要求扁平参数，
// 三 action 共用一个注册面）；核心分派在 run_verify_action，本函数只做
// 参数收口。
#[cfg(feature = "cli")]
#[allow(clippy::too_many_arguments)]
#[forge(
    name = "verify",
    version = "0.4.0",
    description = "Cross-validate CodeNexus parsing against gitnexus reference indexes. Params: action — single|batch|fetch-samples (required); repo — target repo root (single); name — project name (single); language — primary language (single); corpus — samples.json path (batch/fetch-samples, default tools/verification/samples.json); resume — skip re-index when results JSON exists; only — comma-separated sample filter (batch); gitnexus_binary — path to the gitnexus binary (optional).",
    cli = true
)]
async fn verify(
    action: String,
    repo: String,
    name: String,
    language: String,
    corpus: String,
    resume: bool,
    only: String,
    gitnexus_binary: String,
) -> Result<(), ApiError> {
    // verify 不消费 Kit：主入口为它跳过 Kit 构建（见 run_cli 的
    // needs_kit bypass）——single/batch 经子进程自索引，统计经
    // Repository 直开 DB，全程不依赖已装配的能力集。
    let gitnexus: Option<&Path> = if gitnexus_binary.trim().is_empty() {
        None
    } else {
        Some(Path::new(gitnexus_binary.trim()))
    };
    let result = match action.as_str() {
        "single" => {
            if repo.is_empty() || name.is_empty() || language.is_empty() {
                return Err(ApiError::InvalidInput {
                    message: "verify single requires --repo, --name and --language".to_string(),
                    field: Some("action".to_string()),
                    value: Some(serde_json::Value::String(action.clone())),
                });
            }
            run_single_orchestrator(Path::new(&repo), &name, &language, resume, gitnexus)
        }
        "batch" => {
            let corpus = if corpus.trim().is_empty() {
                PathBuf::from("tools/verification/samples.json")
            } else {
                PathBuf::from(corpus.trim())
            };
            let only = parse_only_filter(&only);
            run_batch(&corpus, resume, &only, gitnexus)
        }
        "fetch-samples" => {
            let corpus = if corpus.trim().is_empty() {
                PathBuf::from("tools/verification/samples.json")
            } else {
                PathBuf::from(corpus.trim())
            };
            run_fetch_samples(&corpus)
        }
        other => {
            return Err(ApiError::InvalidInput {
                message: format!(
                    "invalid verify action '{other}' (expected single|batch|fetch-samples)"
                ),
                field: Some("action".to_string()),
                value: Some(serde_json::Value::String(other.to_string())),
            });
        }
    };
    // 进度/结果走 stderr 与产物文件（行为等价）；stdout 收敛为一条 JSON
    // 收据（与主 CLI 的 --format json 契约兼容）。
    result
        .map_err(|e| to_api_error(CodeNexusError::Internal(format!("{e:#}")), "verify_error"))
        .map(|()| {
            let receipt = serde_json::json!({
                "command": "verify",
                "action": action,
                "ok": true,
            });
            let json = serde_json::to_string(&receipt)
                .map_err(|e| wrap_error("JSON serialization failed", e))?;
            println!("{json}");
            Ok::<(), ApiError>(())
        })
        .and_then(std::convert::identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_filter_parses_comma_list() {
        assert_eq!(parse_only_filter("a,b , c"), vec!["a", "b", "c"]);
        assert!(parse_only_filter("").is_empty());
        assert!(parse_only_filter(" , ").is_empty());
        assert_eq!(parse_only_filter("velo"), vec!["velo"]);
    }

    /// `verify` 子命令必须经 `#[forge(cli = true)]` 注册进 inventory，
    /// 与主 CLI 的 dispatch 查找面一致（原独立 bin 无此契约）。
    #[test]
    fn verify_command_registered_via_inventory() {
        let found = sdforge::inventory::iter::<sdforge::cli::CliHandlerRegistration>()
            .any(|h| h.name == "verify");
        assert!(found, "verify subcommand must be inventory-registered");
    }

    /// 参数面等价性守卫：`verify` 注册必须携带全部原独立入口的参数
    /// （action/repo/name/language/corpus/resume/only/gitnexus_binary）。
    #[test]
    fn verify_registration_carries_all_parameters() {
        let reg = sdforge::inventory::iter::<sdforge::cli::CliCommandRegistration>()
            .find(|r| r.name == "verify")
            .expect("verify registration must exist");
        let names: Vec<&str> = reg.args.iter().map(|a| a.name).collect();
        for expected in [
            "action",
            "repo",
            "name",
            "language",
            "corpus",
            "resume",
            "only",
            "gitnexus_binary",
        ] {
            assert!(
                names.contains(&expected),
                "missing arg {expected}: {names:?}"
            );
        }
    }
}
