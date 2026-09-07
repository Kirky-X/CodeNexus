# Verification Report: LAPACK

## Summary

| Metric | Value |
|--------|-------|
| Overall | FAIL |
| Critical discrepancies | 6 |
| Major discrepancies | 2 |
| Minor discrepancies | 2 |
| CodeNexus file_count (by lang) | c=2871, fortran=3548, python=1 |
| gitnexus file_count | 6608 |

## Node Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| enum | 0 | 5 | 100.00% | critical |
| file | 6434 | 6608 | 2.63% | minor |
| function | 6545 | 3476 | 46.89% | major |
| struct | 4 | 0 | 100.00% | critical |

## Edge Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| calls | 23105 | 11812 | 48.88% | major |
| defines | 11349 | 10237 | 9.80% | minor |

## Query Comparison

| Query | Result | Missing in CodeNexus | Missing in gitnexus |
|-------|--------|----------------------|---------------------|
| callees_of_function | CRITICAL | 26 | 3 |
| callers_of_function | CRITICAL | 98 | 38 |
| class_methods | MATCH (0) | 0 | 0 |
| extends_chain | MATCH (0) | 0 | 0 |
| file_contains_symbols | CRITICAL | 200 | 200 |
| function_count_by_file | CRITICAL | 171 | 171 |
| implements_list | MATCH (0) | 0 | 0 |
| imports_of_file | MATCH (103) | 0 | 0 |

## Unmapped Types (Informational)

### CodeNexus-only (not indexed by gitnexus)

| Type | Count |
|------|-------|
| GlobalVar | 35 |
| Macro | 4744 |
| Module | 2 |
| Parameter | 59006 |
| Project | 1 |
| Typedef | 19 |
| Variable | 91415 |

### gitnexus-only (not indexed by CodeNexus)

| Type | Count |
|------|-------|
| Folder | 35 |

### Analysis Artifacts (excluded from comparison)

| Type | CodeNexus | gitnexus |
|------|-----------|----------|
| Community | 0 | 127 |
| Process | 0 | 6 |
