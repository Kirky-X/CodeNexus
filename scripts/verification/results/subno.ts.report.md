# Verification Report: subno.ts

## Summary

| Metric | Value |
|--------|-------|
| Overall | FAIL |
| Critical discrepancies | 8 |
| Major discrepancies | 7 |
| Minor discrepancies | 5 |
| CodeNexus file_count (by lang) | c=9, javascript=6, python=26, rust=21, typescript=124 |
| gitnexus file_count | 244 |

## Node Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| class | 164 | 164 | 0.00% | minor |
| enum | 27 | 14 | 48.15% | major |
| file | 213 | 244 | 12.70% | major |
| function | 535 | 838 | 36.16% | major |
| impl | 22 | 22 | 0.00% | minor |
| interface | 111 | 111 | 0.00% | minor |
| method | 1061 | 682 | 35.72% | major |
| struct | 45 | 58 | 22.41% | major |
| trait | 5 | 5 | 0.00% | minor |

## Edge Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| calls | 1270 | 1641 | 22.61% | major |
| defines | 2400 | 2522 | 4.84% | minor |
| extends | 15 | 17 | 11.76% | major |

## Query Comparison

| Query | Result | Missing in CodeNexus | Missing in gitnexus |
|-------|--------|----------------------|---------------------|
| callees_of_function | CRITICAL | 94 | 3 |
| callers_of_function | CRITICAL | 131 | 52 |
| class_methods | CRITICAL | 189 | 141 |
| extends_chain | CRITICAL | 15 | 165 |
| file_contains_symbols | CRITICAL | 200 | 200 |
| function_count_by_file | CRITICAL | 105 | 197 |
| implements_list | CRITICAL | 0 | 527 |
| imports_of_file | CRITICAL | 61 | 79 |

## Unmapped Types (Informational)

### CodeNexus-only (not indexed by gitnexus)

| Type | Count |
|------|-------|
| Const | 137 |
| GlobalVar | 13 |
| Macro | 25 |
| Module | 19 |
| Parameter | 555 |
| Project | 1 |
| Property | 182 |
| TypeAlias | 44 |
| Typedef | 19 |
| Variable | 1790 |

### gitnexus-only (not indexed by CodeNexus)

| Type | Count |
|------|-------|
| Folder | 75 |

### Analysis Artifacts (excluded from comparison)

| Type | CodeNexus | gitnexus |
|------|-----------|----------|
| Community | 0 | 179 |
| Process | 0 | 300 |
| Route | 0 | 11 |
