# Verification Report: gson

## Summary

| Metric | Value |
|--------|-------|
| Overall | FAIL |
| Critical discrepancies | 4 |
| Major discrepancies | 3 |
| Minor discrepancies | 5 |
| CodeNexus file_count (by lang) | java=0 |
| gitnexus file_count | 292 |

## Node Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| class | 667 | 655 | 1.80% | minor |
| enum | 26 | 26 | 0.00% | minor |
| file | 262 | 292 | 10.27% | major |
| interface | 27 | 27 | 0.00% | minor |
| method | 3378 | 2856 | 15.45% | major |

## Edge Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| calls | 7985 | 6081 | 23.84% | major |
| defines | 720 | 784 | 8.16% | minor |
| extends | 105 | 101 | 3.81% | minor |

## Query Comparison

| Query | Result | Missing in CodeNexus | Missing in gitnexus |
|-------|--------|----------------------|---------------------|
| callees_of_function | MATCH (0) | 0 | 0 |
| callers_of_function | MATCH (0) | 0 | 0 |
| class_methods | CRITICAL | 168 | 159 |
| extends_chain | CRITICAL | 0 | 1 |
| file_contains_symbols | MATCH (0) | 0 | 0 |
| function_count_by_file | MATCH (0) | 0 | 0 |
| implements_list | CRITICAL | 0 | 5 |
| imports_of_file | CRITICAL | 4 | 2 |

## Unmapped Types (Informational)

### CodeNexus-only (not indexed by gitnexus)

| Type | Count |
|------|-------|
| Parameter | 771 |
| Project | 1 |
| Variable | 1386 |

### gitnexus-only (not indexed by CodeNexus)

| Type | Count |
|------|-------|
| Folder | 117 |

### Analysis Artifacts (excluded from comparison)

| Type | CodeNexus | gitnexus |
|------|-----------|----------|
| Community | 0 | 313 |
| Process | 0 | 277 |
