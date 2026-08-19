# Verification Report: panorama

## Summary

| Metric | Value |
|--------|-------|
| Overall | FAIL |
| Critical discrepancies | 8 |
| Major discrepancies | 5 |
| Minor discrepancies | 3 |
| CodeNexus file_count (by lang) | python=439, rust=2, typescript=118 |
| gitnexus file_count | 556 |

## Node Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| class | 1151 | 1122 | 2.52% | minor |
| file | 559 | 556 | 0.54% | minor |
| function | 508 | 4597 | 88.95% | major |
| interface | 117 | 123 | 4.88% | minor |
| method | 4185 | 62 | 98.52% | major |

## Edge Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| calls | 3988 | 6803 | 41.38% | major |
| defines | 6033 | 13256 | 54.49% | major |
| extends | 237 | 48 | 79.75% | major |

## Query Comparison

| Query | Result | Missing in CodeNexus | Missing in gitnexus |
|-------|--------|----------------------|---------------------|
| callees_of_function | CRITICAL | 86 | 37 |
| callers_of_function | CRITICAL | 114 | 79 |
| class_methods | CRITICAL | 57 | 145 |
| extends_chain | CRITICAL | 13 | 159 |
| file_contains_symbols | CRITICAL | 156 | 172 |
| function_count_by_file | CRITICAL | 199 | 199 |
| implements_list | CRITICAL | 0 | 532 |
| imports_of_file | CRITICAL | 64 | 90 |

## Unmapped Types (Informational)

### CodeNexus-only (not indexed by gitnexus)

| Type | Count |
|------|-------|
| Const | 53 |
| Module | 2 |
| Parameter | 895 |
| Project | 1 |
| TypeAlias | 17 |
| Variable | 6581 |

### gitnexus-only (not indexed by CodeNexus)

| Type | Count |
|------|-------|
| Folder | 100 |

### Analysis Artifacts (excluded from comparison)

| Type | CodeNexus | gitnexus |
|------|-----------|----------|
| Community | 0 | 481 |
| Process | 0 | 300 |
| Route | 0 | 82 |
