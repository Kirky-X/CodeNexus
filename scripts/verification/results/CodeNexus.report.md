# Verification Report: CodeNexus

## Summary

| Metric | Value |
|--------|-------|
| Overall | FAIL |
| Critical discrepancies | 6 |
| Major discrepancies | 1 |
| Minor discrepancies | 8 |
| CodeNexus file_count (by lang) | python=1, rust=175 |
| gitnexus file_count | 191 |

## Node Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| class | 2 | 2 | 0.00% | minor |
| enum | 43 | 41 | 4.65% | minor |
| file | 176 | 191 | 7.85% | minor |
| function | 4711 | 4553 | 3.35% | minor |
| impl | 121 | 117 | 3.31% | minor |
| struct | 288 | 278 | 3.47% | minor |
| trait | 19 | 18 | 5.26% | minor |

## Edge Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| calls | 9517 | 8635 | 9.27% | minor |
| defines | 6488 | 4730 | 27.10% | major |

## Query Comparison

| Query | Result | Missing in CodeNexus | Missing in gitnexus |
|-------|--------|----------------------|---------------------|
| callees_of_function | CRITICAL | 0 | 7 |
| callers_of_function | CRITICAL | 2 | 5 |
| class_methods | MATCH (0) | 0 | 0 |
| extends_chain | MATCH (0) | 0 | 0 |
| file_contains_symbols | CRITICAL | 55 | 52 |
| function_count_by_file | CRITICAL | 8 | 7 |
| implements_list | CRITICAL | 11 | 1 |
| imports_of_file | CRITICAL | 4 | 3 |

## Unmapped Types (Informational)

### CodeNexus-only (not indexed by gitnexus)

| Type | Count |
|------|-------|
| Const | 143 |
| Macro | 1 |
| Module | 291 |
| Parameter | 1448 |
| Project | 1 |
| Property | 807 |
| Static | 5 |
| TypeAlias | 72 |
| Variable | 4308 |

### gitnexus-only (not indexed by CodeNexus)

| Type | Count |
|------|-------|
| Folder | 28 |

### Analysis Artifacts (excluded from comparison)

| Type | CodeNexus | gitnexus |
|------|-----------|----------|
| Community | 0 | 278 |
| Process | 0 | 300 |
