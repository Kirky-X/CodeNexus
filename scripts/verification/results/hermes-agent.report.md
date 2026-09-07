# Verification Report: hermes-agent

## Summary

| Metric | Value |
|--------|-------|
| Overall | FAIL |
| Critical discrepancies | 8 |
| Major discrepancies | 6 |
| Minor discrepancies | 2 |
| CodeNexus file_count (by lang) | javascript=3, python=812, typescript=3 |
| gitnexus file_count | 1508 |

## Node Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| class | 2685 | 2592 | 3.46% | minor |
| file | 815 | 1508 | 45.95% | major |
| function | 4402 | 17785 | 75.25% | major |
| interface | 1 | 1 | 0.00% | minor |
| method | 12646 | 9 | 99.93% | major |

## Edge Type Comparison (Comparable Types)

| Type | CodeNexus | gitnexus | Delta | Severity |
|------|-----------|----------|-------|----------|
| calls | 21183 | 28645 | 26.05% | major |
| defines | 19736 | 44511 | 55.66% | major |
| extends | 360 | 99 | 72.50% | major |

## Query Comparison

| Query | Result | Missing in CodeNexus | Missing in gitnexus |
|-------|--------|----------------------|---------------------|
| callees_of_function | CRITICAL | 36 | 33 |
| callers_of_function | CRITICAL | 76 | 103 |
| class_methods | CRITICAL | 9 | 140 |
| extends_chain | CRITICAL | 61 | 129 |
| file_contains_symbols | CRITICAL | 139 | 200 |
| function_count_by_file | CRITICAL | 166 | 166 |
| implements_list | CRITICAL | 0 | 532 |
| imports_of_file | CRITICAL | 45 | 90 |

## Unmapped Types (Informational)

### CodeNexus-only (not indexed by gitnexus)

| Type | Count |
|------|-------|
| Const | 2 |
| Parameter | 4037 |
| Project | 1 |
| Variable | 25106 |

### gitnexus-only (not indexed by CodeNexus)

| Type | Count |
|------|-------|
| Folder | 356 |

### Analysis Artifacts (excluded from comparison)

| Type | CodeNexus | gitnexus |
|------|-----------|----------|
| Community | 0 | 1209 |
| Process | 0 | 300 |
| Route | 0 | 7 |
| Tool | 0 | 10 |
