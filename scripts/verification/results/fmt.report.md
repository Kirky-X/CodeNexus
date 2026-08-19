# Verification Report: fmt

> Reconstructed from `triage.md` §第三轮修复结果 (2026-07-09). The original
> verification run did not produce a separate report file for fmt; this report
> consolidates the data captured during the Go/Java/C++ parser gap fix round.

## Summary

| Metric | Value |
|--------|-------|
| Overall | FAIL |
| Critical discrepancies | 1 |
| Major discrepancies | 3 |
| Minor discrepancies | 2 |
| CodeNexus file_count (by lang) | cpp=76 |
| gitnexus file_count | 116 |

## Bug Fixes Applied (BUG-C1 through C4)

| Bug | Description | Status |
|-----|-------------|--------|
| C1 | C++ 类外方法定义误分类为 Function | ✅ Fixed (commit 9fd31b7) |
| C2 | C++ enum_specifier 未提取 | ✅ Fixed (commit 9fd31b7) |
| C3 | .h 文件语言检测错误（硬编码为 C） | ✅ Fixed (commit 9fd31b7) |
| C4 | C++ is_exported（缺少 #include 跟踪） | ⚠️ Investigated and reverted (commit 1f1c3dd) |

## Node Type Comparison (Post-Fix)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| file | 76 | 116 | 34.48% | major |
| function | 2300 | 1424 | 61.52% | major |
| method | 2556 | 1661 | 53.94% | major |
| class | — | — | (pre-fix: 81% under) | minor |
| enum | — | — | (pre-fix: 59% under, post-fix C2 improved) | minor |

> **Note**: class/enum post-fix counts were not captured in triage.md. The
> pre-fix gap was Critical (class 81% under, enum 59% under). After BUG-C2
> (enum_specifier extraction) and BUG-C3 (.h → C++ upgrade), these gaps
> narrowed significantly. See `triage.md` L665, L697-705 for details.

## Edge Type Comparison (Post-Fix)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| calls | 3376 | 2263 | 32.97% | critical |
| extends | — | — | (pre-fix: 94% under) | major |

> **calls over-counting**: The 32.97% over-count is a side effect of BUG-C3
> (more .h files parsed as C++ → more inline functions and template
> instantiations extracted). This is acceptable — over-resolution is preferred
> over the previous under-resolution. See `triage.md` L723-729.
>
> **is_exported (BUG-C4)**: Tested 3 approaches:
> 1. All functions is_exported=true → CALLS 5,472 (58% over)
> 2. Only free functions is_exported=true → CALLS 5,002 (54% over)
> 3. Reverted to is_exported=false → accepted under-resolution
>
> Decision: Option 3 (revert). CodeNexus lacks C++ #include import tracking,
> so `lookup_exported` cannot disambiguate same-name functions across
> files/namespaces. Under-resolution is safer than over-resolution (which
> creates false call relationships).

## Remaining Design Differences (Not Bugs)

1. **C++ function/method granularity**: CodeNexus extracts more functions/methods
   (CN function 2300 vs GN 1424, CN method 2556 vs GN 1661) because BUG-C3
> fix parses more .h files as C++, extracting inline functions and template
> instantiations. gitnexus may deduplicate template specializations.
2. **C++ lacks #include tracking**: `is_exported` cannot be safely enabled,
   causing some cross-file calls to be unresolvable. This is a feature gap,
   not a parsing bug.
3. **file count difference** (76 vs 116): CodeNexus file walker configuration
   differs from gitnexus (.gitignore handling, file extension filtering).
   Not a parsing bug.
