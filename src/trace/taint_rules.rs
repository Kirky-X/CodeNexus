// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

use serde::Serialize;

use crate::model::Language;

/// Builtin rules version (bump on rule-set changes).
pub const RULES_VERSION: &str = "1";

/// How a pattern matches a candidate node name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pattern {
    /// `name == pattern` or `qualified_name == pattern`.
    Exact,
    /// `qualified_name` ends with `::<pattern>` / `::<pattern>` segments.
    Suffix,
}

/// A taint endpoint pattern (source or sink).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TaintPattern {
    pub kind: Pattern,
    pub name: &'static str,
}

/// Per-language source/sink sets.
#[derive(Debug, Clone, Copy)]
pub struct LangRules {
    pub sources: &'static [TaintPattern],
    pub sinks: &'static [TaintPattern],
}

const fn p(kind: Pattern, name: &'static str) -> TaintPattern {
    TaintPattern { kind, name }
}

/// Builtin rules — five languages (version 1). Sources introduce untrusted
/// data; sinks are dangerous operations consuming it.
pub static BUILTIN_RULES: &[(Language, LangRules)] = &[
    (
        Language::C,
        LangRules {
            sources: &[
                p(Pattern::Exact, "getenv"),
                p(Pattern::Exact, "read"),
                p(Pattern::Exact, "recv"),
                p(Pattern::Exact, "fread"),
                p(Pattern::Exact, "fgets"),
                p(Pattern::Exact, "scanf"),
            ],
            sinks: &[
                p(Pattern::Exact, "system"),
                p(Pattern::Exact, "popen"),
                p(Pattern::Exact, "execve"),
                p(Pattern::Exact, "execl"),
                p(Pattern::Exact, "strcpy"),
                p(Pattern::Exact, "strcat"),
                p(Pattern::Exact, "sprintf"),
                p(Pattern::Exact, "memcpy"),
            ],
        },
    ),
    (
        Language::Python,
        LangRules {
            sources: &[
                p(Pattern::Exact, "input"),
                p(Pattern::Exact, "argv"),
                p(Pattern::Exact, "environ"),
                p(Pattern::Suffix, "request.args"),
                p(Pattern::Suffix, "request.form"),
                p(Pattern::Suffix, "request.data"),
            ],
            sinks: &[
                p(Pattern::Exact, "eval"),
                p(Pattern::Exact, "exec"),
                p(Pattern::Suffix, "os.system"),
                p(Pattern::Suffix, "subprocess.call"),
                p(Pattern::Suffix, "subprocess.run"),
                p(Pattern::Suffix, "subprocess.Popen"),
            ],
        },
    ),
    (
        Language::JavaScript,
        LangRules {
            sources: &[
                p(Pattern::Suffix, "location.href"),
                p(Pattern::Suffix, "document.cookie"),
                p(Pattern::Suffix, "req.query"),
                p(Pattern::Suffix, "req.body"),
            ],
            sinks: &[
                p(Pattern::Exact, "eval"),
                p(Pattern::Exact, "execScript"),
                p(Pattern::Exact, "innerHTML"),
                p(Pattern::Exact, "document.write"),
            ],
        },
    ),
    (
        Language::Php,
        LangRules {
            sources: &[
                p(Pattern::Exact, "$_GET"),
                p(Pattern::Exact, "$_POST"),
                p(Pattern::Exact, "$_REQUEST"),
                p(Pattern::Exact, "$_COOKIE"),
            ],
            sinks: &[
                p(Pattern::Exact, "eval"),
                p(Pattern::Exact, "system"),
                p(Pattern::Exact, "exec"),
                p(Pattern::Exact, "shell_exec"),
                p(Pattern::Exact, "passthru"),
            ],
        },
    ),
    (
        Language::Solidity,
        LangRules {
            sources: &[
                p(Pattern::Exact, "msg.sender"),
                p(Pattern::Exact, "msg.value"),
                p(Pattern::Exact, "tx.origin"),
                p(Pattern::Exact, "msg.data"),
            ],
            sinks: &[
                p(Pattern::Exact, "call"),
                p(Pattern::Exact, "delegatecall"),
                p(Pattern::Exact, "selfdestruct"),
                p(Pattern::Exact, "transfer"),
                p(Pattern::Exact, "send"),
            ],
        },
    ),
];

/// A node that matched a taint endpoint pattern.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaintHit {
    pub node_id: String,
    pub name: String,
    pub qualified_name: String,
    /// The pattern that matched.
    pub pattern: String,
}

/// Returns the builtin rules for `language`, if any.
#[must_use]
pub fn rules_for(language: Language) -> Option<&'static LangRules> {
    BUILTIN_RULES
        .iter()
        .find(|(lang, _)| *lang == language)
        .map(|(_, rules)| rules)
}

/// Checks whether a candidate node matches a single pattern.
#[must_use]
pub fn matches_pattern(pattern: &TaintPattern, name: &str, qualified_name: &str) -> bool {
    match pattern.kind {
        Pattern::Exact => name == pattern.name || qualified_name == pattern.name,
        Pattern::Suffix => {
            qualified_name.ends_with(pattern.name)
                && (qualified_name.len() == pattern.name.len()
                    || qualified_name[qualified_name.len() - pattern.name.len() - 1..]
                        .starts_with([':', '.']))
        }
    }
}

/// Matches `name`/`qualified_name` against a rule set, returning the
/// matching patterns.
#[must_use]
pub fn match_node(
    name: &str,
    qualified_name: &str,
    _rules: &LangRules,
    side: &'static [TaintPattern],
) -> Vec<&'static TaintPattern> {
    side.iter()
        .filter(|p| matches_pattern(p, name, qualified_name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_has_nonempty_sources_and_sinks() {
        for (lang, rules) in BUILTIN_RULES {
            assert!(!rules.sources.is_empty(), "{lang} has no sources");
            assert!(!rules.sinks.is_empty(), "{lang} has no sinks");
        }
        assert_eq!(BUILTIN_RULES.len(), 5);
    }

    #[test]
    fn c_sources_and_sinks_match_and_reject() {
        let rules = rules_for(Language::C).unwrap();
        let getenv = rules.sources.iter().find(|p| p.name == "getenv").unwrap();
        assert!(matches_pattern(getenv, "getenv", "libc.getenv"));
        assert!(matches_pattern(getenv, "getenv", "getenv"));
        // Negative: a different function with a similar prefix.
        assert!(!matches_pattern(getenv, "getenv_ext", "libc.getenv_ext"));
        let strcpy = rules.sinks.iter().find(|p| p.name == "strcpy").unwrap();
        assert!(matches_pattern(strcpy, "strcpy", "libc.strcpy"));
        assert!(!matches_pattern(strcpy, "fgets", "libc.fgets"));
    }

    #[test]
    fn python_suffix_matches_segment_boundary() {
        let rules = rules_for(Language::Python).unwrap();
        let sys = rules.sinks.iter().find(|p| p.name == "os.system").unwrap();
        assert!(matches_pattern(sys, "system", "os.system"));
        assert!(matches_pattern(sys, "system", "lib.os.system"));
        // Segment boundary: `os.systematic` must NOT match.
        assert!(!matches_pattern(sys, "systematic", "os.systematic"));
        assert!(!matches_pattern(sys, "systematic", "lib.os.systematic"));
    }

    #[test]
    fn php_superglobals_match() {
        let rules = rules_for(Language::Php).unwrap();
        let get = rules.sources.iter().find(|p| p.name == "$_GET").unwrap();
        assert!(matches_pattern(get, "$_GET", "$_GET"));
        assert!(matches_pattern(get, "$_GET", "app.$_GET"));
    }

    #[test]
    fn solidity_sources_match_msg_senders() {
        let rules = rules_for(Language::Solidity).unwrap();
        let sender = rules
            .sources
            .iter()
            .find(|p| p.name == "msg.sender")
            .unwrap();
        assert!(matches_pattern(sender, "sender", "msg.sender"));
        assert!(matches_pattern(sender, "msg.sender", "msg.sender"));
    }

    #[test]
    fn match_node_collects_all_hits() {
        let rules = rules_for(Language::C).unwrap();
        let hits = match_node("getenv", "libc.getenv", rules, rules.sources);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "getenv");
        assert!(match_node("foo", "bar.foo", rules, rules.sources).is_empty());
    }

    #[test]
    fn unknown_language_has_no_rules() {
        assert!(rules_for(Language::Haskell).is_none());
    }
}
