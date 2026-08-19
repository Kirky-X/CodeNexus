# Verification Report: claudecode

## Summary

| Metric | Value |
|--------|-------|
| Overall | FAIL |
| Critical discrepancies | 10 |
| Major discrepancies | 3 |
| Minor discrepancies | 4 |
| CodeNexus file_count (by lang) | typescript=1906 |
| gitnexus file_count | 1921 |

## Node Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| class | 128 | 128 | 0.00% | minor |
| enum | 1 | 0 | 100.00% | critical |
| file | 1906 | 1921 | 0.78% | minor |
| function | 9010 | 9412 | 4.27% | minor |
| interface | 83 | 72 | 13.25% | major |
| method | 2034 | 1804 | 11.31% | major |

## Edge Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| calls | 24607 | 23221 | 5.63% | minor |
| defines | 16969 | 12046 | 29.01% | major |
| extends | 0 | 10 | 100.00% | critical |

## Query Comparison

| Query | Result | Missing in CodeNexus | Missing in gitnexus |
|-------|--------|----------------------|---------------------|
| callees_of_function | CRITICAL | 106 | 3 |
| callers_of_function | CRITICAL | 14 | 29 |
| class_methods | CRITICAL | 175 | 141 |
| extends_chain | CRITICAL | 10 | 167 |
| file_contains_symbols | CRITICAL | 151 | 151 |
| function_count_by_file | CRITICAL | 167 | 167 |
| implements_list | CRITICAL | 3 | 532 |
| imports_of_file | CRITICAL | 150 | 75 |

## Unmapped Types (Informational)

### CodeNexus-only (not indexed by gitnexus)

| Type | Count |
|------|-------|
| Const | 3304 |
| Parameter | 7831 |
| Project | 1 |
| TypeAlias | 2435 |
| Variable | 32395 |

### gitnexus-only (not indexed by CodeNexus)

| Type | Count |
|------|-------|
| Folder | 282 |

### Analysis Artifacts (excluded from comparison)

| Type | CodeNexus | gitnexus |
|------|-----------|----------|
| Community | 0 | 1083 |
| Process | 0 | 300 |
