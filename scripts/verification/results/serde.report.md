# Verification Report: serde

## Summary

| Metric | Value |
|--------|-------|
| Overall | FAIL |
| Critical discrepancies | 5 |
| Major discrepancies | 4 |
| Minor discrepancies | 4 |
| CodeNexus file_count (by lang) | rust=188 |
| gitnexus file_count | 324 |

## Node Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| enum | 194 | 180 | 7.22% | minor |
| file | 188 | 324 | 41.98% | major |
| function | 1669 | 1474 | 11.68% | major |
| impl | 66 | 66 | 0.00% | minor |
| struct | 440 | 426 | 3.18% | minor |
| trait | 38 | 38 | 0.00% | minor |

## Edge Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| calls | 1655 | 1391 | 15.95% | major |
| defines | 3306 | 1693 | 48.79% | major |

## Query Comparison

| Query | Result | Missing in CodeNexus | Missing in gitnexus |
|-------|--------|----------------------|---------------------|
| callees_of_function | CRITICAL | 32 | 16 |
| callers_of_function | CRITICAL | 30 | 14 |
| class_methods | MATCH (0) | 0 | 0 |
| extends_chain | MATCH (0) | 0 | 0 |
| file_contains_symbols | CRITICAL | 141 | 57 |
| function_count_by_file | MATCH (177) | 0 | 0 |
| implements_list | CRITICAL | 32 | 5 |
| imports_of_file | CRITICAL | 25 | 0 |

## Unmapped Types (Informational)

### CodeNexus-only (not indexed by gitnexus)

| Type | Count |
|------|-------|
| Const | 51 |
| Macro | 58 |
| Module | 90 |
| Parameter | 521 |
| Project | 1 |
| Property | 534 |
| Static | 3 |
| TypeAlias | 208 |
| Variable | 741 |

### gitnexus-only (not indexed by CodeNexus)

| Type | Count |
|------|-------|
| Folder | 37 |

### Analysis Artifacts (excluded from comparison)

| Type | CodeNexus | gitnexus |
|------|-----------|----------|
| Community | 0 | 48 |
| Process | 0 | 300 |
