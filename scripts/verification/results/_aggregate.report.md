# Aggregate Verification Report

## Per-Sample Summary

| Sample | Language | Overall | Critical | Major | Minor |
|--------|----------|---------|----------|-------|-------|
| CodeNexus | rust | FAIL | 6 | 1 | 8 |
| redis | c | FAIL | 6 | 7 | 2 |
| LAPACK | fortran | FAIL | 6 | 2 | 2 |
| cobra | go | FAIL | 4 | 3 | 4 |
| gson | java | FAIL | 4 | 3 | 5 |

## Ranking (by critical count desc)

1. **redis** — critical=6, major=7, minor=2, overall=FAIL
2. **LAPACK** — critical=6, major=2, minor=2, overall=FAIL
3. **CodeNexus** — critical=6, major=1, minor=8, overall=FAIL
4. **gson** — critical=4, major=3, minor=5, overall=FAIL
5. **cobra** — critical=4, major=3, minor=4, overall=FAIL

**Total: 0 pass, 5 fail (of 5 samples)**
