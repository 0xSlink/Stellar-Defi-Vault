#!/usr/bin/env python3
"""Verify that every test referenced in docs/TRACEABILITY.md exists in the codebase."""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TRACEABILITY_DOC = ROOT / "docs" / "TRACEABILITY.md"
TEST_REF_RE = re.compile(r"`([^`]+?)::([A-Za-z0-9_]+)`")


def main() -> int:
    if not TRACEABILITY_DOC.is_file():
        print(f"ERROR: Missing traceability matrix at {TRACEABILITY_DOC}", file=sys.stderr)
        return 1

    content = TRACEABILITY_DOC.read_text(encoding="utf-8")
    errors: list[str] = []
    checked = 0

    for line_no, line in enumerate(content.splitlines(), start=1):
        if not line.startswith("|"):
            continue
        cells = [cell.strip() for cell in line.strip("|").split("|")]
        if len(cells) < 5:
            continue
        threat_id, _threat, _mitigation, test_cell, status = cells[:5]
        if threat_id in {"ID", "---"} or threat_id.startswith("-"):
            continue

        refs = TEST_REF_RE.findall(test_cell)
        if status.lower() == "covered" and not refs:
            errors.append(
                f"line {line_no} ({threat_id}): status is 'Covered' but no `file::test_name` references found"
            )
            continue

        for rel_path, fn_name in refs:
            checked += 1
            file_path = ROOT / rel_path
            if not file_path.is_file():
                errors.append(
                    f"line {line_no} ({threat_id}): referenced test file does not exist: {rel_path}"
                )
                continue
            source = file_path.read_text(encoding="utf-8")
            fn_pattern = re.compile(rf"\bfn\s+{re.escape(fn_name)}\s*\(")
            if not fn_pattern.search(source):
                errors.append(
                    f"line {line_no} ({threat_id}): test function '{fn_name}' not found in {rel_path}"
                )

    if checked == 0:
        print("ERROR: No test references found in docs/TRACEABILITY.md", file=sys.stderr)
        return 1

    if errors:
        print("Traceability verification failed:", file=sys.stderr)
        for err in errors:
            print(f"  - {err}", file=sys.stderr)
        return 1

    print(f"OK: verified {checked} test reference(s) in {TRACEABILITY_DOC.relative_to(ROOT)}.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
