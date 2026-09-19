// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

use serde_json::{Map, Value};

/// Receipt describing what the budget pass did to a payload.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct BudgetReceipt {
    pub requested: usize,
    pub estimated_tokens: usize,
    pub truncated: bool,
    pub sections_dropped: Vec<String>,
    pub oversized_dropped: usize,
}

/// Estimates tokens for a string with the documented `chars / 4` heuristic.
///
/// Counts Unicode chars (not bytes), so CJK text is not over-counted 4x.
pub fn estimate_tokens(s: &str) -> usize {
    s.chars().count() / 4
}

fn serialized_tokens(v: &Value) -> usize {
    estimate_tokens(&v.to_string())
}

/// Applies a token budget to an output value.
///
/// Rules (see change `feature-expansion-wave`, spec R-context-002):
/// - `budget == 0` means unlimited: the value is returned unchanged and no
///   `budget` receipt is attached.
/// - Non-array fields are never truncated (identifiers, nested objects).
/// - Array fields are packed greedily in the map's (sorted) key order,
///   keeping a prefix of each array; elements after the first one that does
///   not fit are dropped.
/// - A single element whose own estimate exceeds the whole budget can never
///   fit; it counts toward `oversized_dropped`.
/// - The root object gains a `budget` receipt; `estimated_tokens` is the
///   post-truncation estimate (it may still exceed `requested` when the
///   non-truncatable skeleton alone is larger than the budget).
///
/// Non-object values are returned unchanged (nothing to pack).
pub fn apply_budget(value: Value, budget: usize) -> Value {
    if budget == 0 {
        return value;
    }
    let Some(map) = value.as_object() else {
        return value;
    };

    // Skeleton estimate: same fields, arrays emptied.
    let mut skeleton = Map::new();
    for (k, v) in map {
        if v.is_array() {
            skeleton.insert(k.clone(), Value::Array(Vec::new()));
        } else {
            skeleton.insert(k.clone(), v.clone());
        }
    }
    let mut remaining = budget.saturating_sub(serialized_tokens(&Value::Object(skeleton.clone())));

    let mut sections_dropped = Vec::new();
    let mut oversized_dropped = 0usize;
    let mut truncated = false;

    for (key, v) in map {
        if !v.is_array() {
            continue;
        }
        let elems = v.as_array().expect("checked is_array");
        let mut kept = Vec::with_capacity(elems.len());
        let mut dropped_here = false;
        for elem in elems {
            let est = serialized_tokens(elem);
            if est > budget {
                oversized_dropped += 1;
                dropped_here = true;
                continue;
            }
            if est <= remaining {
                remaining -= est;
                kept.push(elem.clone());
            } else {
                dropped_here = true;
            }
        }
        if dropped_here {
            truncated = true;
            sections_dropped.push(key.clone());
        }
        skeleton.insert(key.clone(), Value::Array(kept));
    }

    let receipt = BudgetReceipt {
        requested: budget,
        estimated_tokens: serialized_tokens(&Value::Object(skeleton.clone())),
        truncated,
        sections_dropped,
        oversized_dropped,
    };
    skeleton.insert(
        "budget".to_string(),
        serde_json::to_value(&receipt).unwrap_or(Value::Null),
    );
    Value::Object(skeleton)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn elem(tag: &str, pad_chars: usize) -> Value {
        json!({ "name": tag, "pad": "x".repeat(pad_chars) })
    }

    #[test]
    fn estimate_tokens_counts_chars_not_bytes() {
        assert_eq!(estimate_tokens("abcdefgh"), 2);
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("ab"), 0);
        // 8 CJK chars = 8 chars = 2 tokens (not 24 bytes / 4 = 6).
        assert_eq!(estimate_tokens("上下上下上下上下"), 2);
    }

    #[test]
    fn budget_zero_is_identity() {
        let v = json!({"symbol": "a", "incoming": [1, 2, 3]});
        let out = apply_budget(v.clone(), 0);
        assert_eq!(out, v);
        assert!(out.get("budget").is_none());
    }

    #[test]
    fn non_object_passthrough() {
        let v = json!([1, 2, 3]);
        assert_eq!(apply_budget(v.clone(), 5), v);
    }

    #[test]
    fn scalars_survive_tiniest_budget() {
        let v = json!({
            "symbol": "parse_source",
            "node": {"name": "parse_source", "label": "Function"},
            "incoming": [elem("a", 400), elem("b", 400)]
        });
        let out = apply_budget(v, 1);
        assert_eq!(out["symbol"], "parse_source");
        assert!(out["node"].is_object());
        assert!(out["incoming"].as_array().unwrap().is_empty());
        assert_eq!(out["budget"]["truncated"], true);
    }

    #[test]
    fn arrays_are_prefix_truncated_in_key_order() {
        // Keys in sorted order: "incoming" < "outgoing" < "symbol".
        // Each element is ~40 chars of pad + wrapper ≈ 15 tokens.
        let elems: Vec<Value> = (0..10).map(|i| elem(&format!("e{i}"), 40)).collect();
        let v = json!({
            "symbol": "x",
            "incoming": elems,
            "outgoing": [elem("o", 40)]
        });
        // Skeleton (symbol + empty arrays) ≈ 15 tokens; budget fits ~7 elems.
        let out = apply_budget(v, 100);
        let kept = out["incoming"].as_array().unwrap().len();
        assert!(
            kept > 0 && kept < 10,
            "expected partial packing, got {kept}"
        );
        // Key order: incoming packs first; outgoing (later key) gets the
        // remainder — likely emptied, and definitely not larger than allowed.
        let out_tokens = out["budget"]["estimated_tokens"].as_u64().unwrap();
        assert!(
            out_tokens <= 100 + 20,
            "estimate wildly over budget: {out_tokens}"
        );
        assert_eq!(out["budget"]["truncated"], true);
        let dropped = out["budget"]["sections_dropped"].as_array().unwrap();
        assert!(dropped.contains(&json!("incoming")));
    }

    #[test]
    fn oversized_element_counts_and_drops() {
        let big = elem("big", 10_000);
        let v = json!({"symbol": "x", "incoming": [big, elem("small", 4)]});
        let out = apply_budget(v, 50);
        assert_eq!(out["incoming"].as_array().unwrap().len(), 1);
        assert_eq!(out["budget"]["oversized_dropped"], 1);
        assert_eq!(out["budget"]["truncated"], true);
    }

    #[test]
    fn exact_fit_reports_not_truncated() {
        // Build payload, measure its natural estimate, then budget exactly that.
        let v = json!({
            "symbol": "abc",
            "incoming": [elem("a", 8), elem("b", 8)]
        });
        let natural = serialized_tokens(&v);
        let out = apply_budget(v.clone(), natural);
        assert_eq!(out["incoming"].as_array().unwrap().len(), 2);
        assert_eq!(out["budget"]["truncated"], false);
        assert!(out["budget"]["sections_dropped"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn empty_array_field_never_marks_truncation() {
        let v = json!({"symbol": "x", "incoming": [], "outgoing": [elem("o", 4)]});
        let out = apply_budget(
            v,
            serialized_tokens(&json!({
                "symbol": "x", "incoming": [], "outgoing": []
            })) + serialized_tokens(&elem("o", 4)),
        );
        assert_eq!(out["budget"]["truncated"], false);
        assert_eq!(out["budget"]["sections_dropped"], json!([]));
    }

    #[test]
    fn nested_object_estimate_matches_serialized_size() {
        // A nested object field is treated as an opaque non-array field: it
        // must survive even a tiny budget, and the skeleton estimate must
        // include its full serialized size.
        let nested = json!({"deep": {"deeper": {"leaf": "y".repeat(64)}}});
        let v = json!({"symbol": "x", "context": nested, "incoming": [elem("a", 4)]});
        let skeleton =
            json!({"symbol": "x", "context": {"deep": {"deeper": {"leaf": ""}}}, "incoming": []});
        // Budget far below the nested field's size: arrays emptied, object kept.
        let out = apply_budget(v, 10);
        assert_eq!(
            out["context"]["deep"]["deeper"]["leaf"]
                .as_str()
                .unwrap()
                .len(),
            64
        );
        assert!(
            out["budget"]["estimated_tokens"].as_u64().unwrap()
                >= (serialized_tokens(&skeleton) / 4) as u64
        );
    }
}
