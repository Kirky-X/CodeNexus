# Verification Report: velo

## Summary

| Metric | Value |
|--------|-------|
| Overall | FAIL |
| Critical discrepancies | 8 |
| Major discrepancies | 2 |
| Minor discrepancies | 6 |
| CodeNexus file_count (by lang) | python=3, rust=278 |
| gitnexus file_count | 291 |

## Node Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| enum | 187 | 186 | 0.53% | minor |
| file | 281 | 291 | 3.44% | minor |
| function | 7050 | 6252 | 11.32% | major |
| impl | 633 | 625 | 1.26% | minor |
| struct | 842 | 828 | 1.66% | minor |
| trait | 41 | 41 | 0.00% | minor |

## Edge Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| calls | 11264 | 7080 | 37.14% | major |
| defines | 11979 | 11243 | 6.14% | minor |

## Query Comparison

| Query | Result | Missing in CodeNexus | Missing in gitnexus |
|-------|--------|----------------------|---------------------|
| callees_of_function | CRITICAL | 48 | 23 |
| callers_of_function | CRITICAL | 91 | 53 |
| class_methods | CRITICAL | 0 | 151 |
| extends_chain | CRITICAL | 0 | 15 |
| file_contains_symbols | CRITICAL | 94 | 72 |
| function_count_by_file | CRITICAL | 76 | 76 |
| implements_list | CRITICAL | 0 | 395 |
| imports_of_file | CRITICAL | 39 | 40 |

## Unmapped Types (Informational)

### CodeNexus-only (not indexed by gitnexus)

| Type | Count |
|------|-------|
| Const | 84 |
| Module | 574 |
| Parameter | 1364 |
| Project | 1 |
| Property | 2894 |
| Static | 10 |
| TypeAlias | 78 |
| Variable | 4533 |

### gitnexus-only (not indexed by CodeNexus)

| Type | Count |
|------|-------|
| Folder | 39 |

### Analysis Artifacts (excluded from comparison)

| Type | CodeNexus | gitnexus |
|------|-----------|----------|
| Community | 0 | 691 |
| Process | 0 | 300 |
