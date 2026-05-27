#!/usr/bin/env python3
"""Smoke harness for libflutter_engine.so patch verification.

Run: python3 harness/verify_engine_patches.py
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ENGINE = ROOT / "tools/flutter-engine/libflutter_engine.so"
PATCH_TOOL = ROOT / "tools/flutter-engine/engine_patch.py"


def main() -> int:
    failed = 0
    passed = 0

    if not ENGINE.exists():
        print(f"FAIL missing engine binary: {ENGINE}")
        return 1

    size_mb = ENGINE.stat().st_size / (1024 * 1024)
    if size_mb < 50:
        print(f"FAIL engine looks too small ({size_mb:.1f} MB)")
        failed += 1
    else:
        print(f"OK engine size {size_mb:.1f} MB")
        passed += 1

    proc = subprocess.run(
        [sys.executable, str(PATCH_TOOL), "--verify"],
        capture_output=True,
        text=True,
    )
    if proc.returncode == 0:
        print("OK P1–P6, P9, P10 engine patches present")
        passed += 1
    else:
        print("FAIL patch verification:")
        print(proc.stdout)
        print(proc.stderr)
        failed += 1

    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
