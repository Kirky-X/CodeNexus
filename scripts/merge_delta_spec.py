#!/usr/bin/env python3
# Copyright (c) 2026 Kirky.X. All rights reserved.
# SPDX-License-Identifier: MIT
"""Deterministic delta spec merger for specmark workflow.

Merges a delta spec into a main spec by R-<cap>-NNN key:
  ADD    — R-ID only in delta: append (sorted by numeric suffix)
  MODIFY — R-ID in both: delta title+body replaces main
  DELETE — delta title is ~~DELETE~~: discard that R-ID
  KEEP   — R-ID only in main: keep as-is

Constraints / Out of Scope: line union (main first, delta-unique lines appended).
Idempotent: merging (main, delta) twice yields identical bytes.

Usage:
  python3 scripts/merge_delta_spec.py \
      --main specmark/specs/<cap>/spec.md \
      --delta specmark/changes/<name>/specs/<cap>/spec.md \
      [--dry-run]
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

R_ID_RE = re.compile(r"^###\s+(R-[A-Za-z0-9_]+-(\d+)):\s*(.*)$")
DELETE_MARK = "~~DELETE~~"


@dataclass
class Requirement:
    r_id: str
    seq: int
    title: str
    body: str  # includes trailing newline; lines after header until next ###/##


@dataclass
class Spec:
    header: str  # "# Spec — <cap>" line + any intro lines up to "## Requirements"
    requirements: list[Requirement] = field(default_factory=list)
    constraints: list[str] = field(default_factory=list)
    out_of_scope: list[str] = field(default_factory=list)
    # raw trailing content after Out of Scope section (if any)
    trailing: str = ""


def parse_spec(text: str) -> Spec:
    lines = text.splitlines(keepends=True)
    spec = Spec(header="")
    i = 0
    n = len(lines)

    # header: everything up to (but not including) "## Requirements"
    while i < n:
        if lines[i].startswith("## Requirements"):
            break
        spec.header += lines[i]
        i += 1
    # skip "## Requirements\n"
    if i < n:
        i += 1  # consume the "## Requirements" line

    # parse requirements until "## Constraints" or "## Out of Scope" or EOF
    while i < n:
        line = lines[i]
        if line.startswith("## "):
            break
        m = R_ID_RE.match(line.rstrip("\n"))
        if m:
            r_id = m.group(1)
            seq = int(m.group(2))
            title = m.group(3)
            i += 1
            body = ""
            while i < n and not lines[i].startswith("### ") and not lines[i].startswith("## "):
                body += lines[i]
                i += 1
            spec.requirements.append(Requirement(r_id=r_id, seq=seq, title=title, body=body))
        else:
            i += 1  # skip non-matching lines in requirements section

    # parse "## Constraints" if present
    if i < n and lines[i].startswith("## Constraints"):
        i += 1  # consume header
        while i < n and not lines[i].startswith("## "):
            spec.constraints.append(lines[i])
            i += 1

    # parse "## Out of Scope" if present
    if i < n and lines[i].startswith("## Out of Scope"):
        i += 1  # consume header
        while i < n and not lines[i].startswith("## "):
            spec.out_of_scope.append(lines[i])
            i += 1

    # trailing content
    while i < n:
        spec.trailing += lines[i]
        i += 1

    return spec


def merge_requirements(
    main_reqs: list[Requirement], delta_reqs: list[Requirement]
) -> list[Requirement]:
    main_map = {r.r_id: r for r in main_reqs}
    result_map: dict[str, Requirement] = {}

    # KEEP: main-only
    for r in main_reqs:
        result_map[r.r_id] = r

    for dr in delta_reqs:
        if dr.title.strip() == DELETE_MARK:
            # DELETE
            result_map.pop(dr.r_id, None)
        elif dr.r_id in main_map:
            # MODIFY: delta replaces main (keep delta's title+body)
            result_map[dr.r_id] = dr
        else:
            # ADD
            result_map[dr.r_id] = dr

    # sort by numeric sequence suffix
    return sorted(result_map.values(), key=lambda r: r.seq)


def merge_lines(main_lines: list[str], delta_lines: list[str]) -> list[str]:
    main_set = {ln.rstrip("\n") for ln in main_lines}
    result = list(main_lines)
    for ln in delta_lines:
        if ln.rstrip("\n") not in main_set:
            result.append(ln)
    return result


def serialize(spec: Spec) -> str:
    out = []
    out.append(spec.header)
    if not spec.header.endswith("\n") and spec.header:
        out.append("\n")
    out.append("## Requirements\n\n")
    for r in spec.requirements:
        out.append(f"### {r.r_id}: {r.title}\n")
        if r.body and not r.body.startswith("\n"):
            out.append("\n")
        out.append(r.body)
        if r.body and not r.body.endswith("\n"):
            out.append("\n")
    if spec.constraints:
        out.append("## Constraints\n")
        out.extend(spec.constraints)
    if spec.out_of_scope:
        out.append("## Out of Scope\n")
        out.extend(spec.out_of_scope)
    out.append(spec.trailing)
    return "".join(out)


def merge(main_text: str, delta_text: str) -> str:
    main_spec = parse_spec(main_text)
    delta_spec = parse_spec(delta_text)

    merged_reqs = merge_requirements(main_spec.requirements, delta_spec.requirements)
    merged_constraints = merge_lines(main_spec.constraints, delta_spec.constraints)
    merged_oos = merge_lines(main_spec.out_of_scope, delta_spec.out_of_scope)

    # Use delta header if main empty (ADD case for new capability)
    header = main_spec.header if main_spec.header.strip() else delta_spec.header
    trailing = main_spec.trailing or delta_spec.trailing

    result = Spec(
        header=header,
        requirements=merged_reqs,
        constraints=merged_constraints,
        out_of_scope=merged_oos,
        trailing=trailing,
    )
    return serialize(result)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--main", required=True, type=Path, help="main spec path")
    ap.add_argument("--delta", required=True, type=Path, help="delta spec path")
    ap.add_argument("--dry-run", action="store_true", help="preview without writing")
    args = ap.parse_args()

    delta_text = args.delta.read_text(encoding="utf-8")
    if args.main.exists():
        main_text = args.main.read_text(encoding="utf-8")
    else:
        main_text = ""  # new capability: all ADD

    merged = merge(main_text, delta_text)

    if args.dry_run:
        sys.stdout.write(merged)
        if not merged.endswith("\n"):
            sys.stdout.write("\n")
        return 0

    args.main.parent.mkdir(parents=True, exist_ok=True)
    args.main.write_text(merged, encoding="utf-8")
    print(f"merged -> {args.main}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
