# Verification Report: redis

## Summary

| Metric | Value |
|--------|-------|
| Overall | FAIL |
| Critical discrepancies | 6 |
| Major discrepancies | 7 |
| Minor discrepancies | 2 |
| CodeNexus file_count (by lang) | c=784, javascript=1, python=46 |
| gitnexus file_count | 1772 |

## Node Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| class | 76 | 59 | 22.37% | major |
| enum | 85 | 125 | 32.00% | major |
| file | 1384 | 1772 | 21.90% | major |
| function | 11666 | 13545 | 13.87% | major |
| method | 295 | 355 | 16.90% | major |
| struct | 738 | 1204 | 38.70% | major |

## Edge Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| calls | 13077 | 24318 | 46.23% | major |
| defines | 25827 | 23407 | 9.37% | minor |
| extends | 42 | 42 | 0.00% | minor |

## Query Comparison

| Query | Result | Missing in CodeNexus | Missing in gitnexus |
|-------|--------|----------------------|---------------------|
| callees_of_function | CRITICAL | 14 | 17 |
| callers_of_function | CRITICAL | 21 | 71 |
| class_methods | CRITICAL | 44 | 57 |
| extends_chain | MATCH (42) | 0 | 0 |
| file_contains_symbols | CRITICAL | 27 | 22 |
| function_count_by_file | CRITICAL | 43 | 43 |
| implements_list | MATCH (0) | 0 | 0 |
| imports_of_file | CRITICAL | 28 | 21 |

## Unmapped Types (Informational)

### CodeNexus-only (not indexed by gitnexus)

| Type | Count |
|------|-------|
| GlobalVar | 991 |
| Macro | 4791 |
| Parameter | 14574 |
| Project | 1 |
| Property | 6465 |
| Typedef | 715 |
| Variable | 12552 |

### gitnexus-only (not indexed by CodeNexus)

| Type | Count |
|------|-------|
| Folder | 93 |

### Analysis Artifacts (excluded from comparison)

| Type | CodeNexus | gitnexus |
|------|-----------|----------|
| Community | 0 | 865 |
| Process | 0 | 300 |
