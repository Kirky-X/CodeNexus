# Verification Report: cobra

## Summary

| Metric | Value |
|--------|-------|
| Overall | FAIL |
| Critical discrepancies | 4 |
| Major discrepancies | 3 |
| Minor discrepancies | 4 |
| CodeNexus file_count (by lang) | go=0 |
| gitnexus file_count | 54 |

## Node Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| file | 36 | 54 | 33.33% | major |
| function | 427 | 427 | 0.00% | minor |
| interface | 1 | 1 | 0.00% | minor |
| method | 168 | 169 | 0.59% | minor |
| struct | 14 | 11 | 21.43% | major |

## Edge Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| calls | 1493 | 1922 | 22.32% | major |
| defines | 617 | 564 | 8.59% | minor |

## Query Comparison

| Query | Result | Missing in CodeNexus | Missing in gitnexus |
|-------|--------|----------------------|---------------------|
| callees_of_function | CRITICAL | 0 | 3 |
| callers_of_function | CRITICAL | 50 | 7 |
| class_methods | CRITICAL | 1 | 49 |
| extends_chain | MATCH (0) | 0 | 0 |
| file_contains_symbols | MATCH (200) | 0 | 0 |
| function_count_by_file | MATCH (35) | 0 | 0 |
| implements_list | MATCH (0) | 0 | 0 |
| imports_of_file | CRITICAL | 20 | 0 |

## Unmapped Types (Informational)

### CodeNexus-only (not indexed by gitnexus)

| Type | Count |
|------|-------|
| Parameter | 289 |
| Project | 1 |
| TypeAlias | 7 |
| Variable | 328 |

### gitnexus-only (not indexed by CodeNexus)

| Type | Count |
|------|-------|
| Folder | 5 |

### Analysis Artifacts (excluded from comparison)

| Type | CodeNexus | gitnexus |
|------|-----------|----------|
| Community | 0 | 53 |
| Process | 0 | 83 |
