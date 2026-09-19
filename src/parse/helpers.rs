// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Shared helpers used by all language extractors.

use tree_sitter::Node;

use crate::ir::ExtractResult;

/// Returns the UTF-8 text slice of `node` within `source`, or `None` when the
/// node's bytes are not valid UTF-8.
///
/// Shared by all language extractors — previously each of the 20 extractor
/// files carried a byte-identical private copy of this function.
pub(crate) fn node_text<'a>(node: Node<'a>, source: &'a str) -> Option<&'a str> {
    node.utf8_text(source.as_bytes()).ok()
}

/// Returns a de-duplicated qualified name, appending `#L{line}` if `qn` has
/// already been registered in `result.seen_qns`.
///
/// Previously each extractor had its own O(N) implementation that scanned
/// `result.nodes` linearly on every call, making total extraction O(N²). This
/// shared version consults the O(1) `seen_qns` HashSet maintained by
/// [`ExtractResult::push_node`].
///
/// # Contract
///
/// The caller must push the resulting node via
/// [`ExtractResult::push_node`] (not `result.nodes.push(...))`), so that the
/// returned FQN is registered for future de-duplication.
#[must_use]
pub fn dedupe_qn(qn: String, line: u32, result: &ExtractResult) -> String {
    if result.seen_qns.contains(&qn) {
        format!("{qn}#L{line}")
    } else {
        qn
    }
}
