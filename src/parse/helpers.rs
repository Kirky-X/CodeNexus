// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Shared helpers used by all language extractors.

use crate::ir::ExtractResult;

/// Returns a de-duplicated qualified name, appending `#L{line}` if `qn` has
/// already been registered in `result.seen_qns` (MED-002).
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
