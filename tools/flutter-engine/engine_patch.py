#!/usr/bin/env python3
"""OSCortex Flutter engine binary patch utilities.

All patches target tools/flutter-engine/libflutter_engine.so (copied to
initramfs by scripts/build-iso.sh on every build).
"""

from __future__ import annotations

import argparse
import shutil
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_ENGINE = ROOT / "tools/flutter-engine/libflutter_engine.so"

LOAD_SEGMENTS = (
    (0x0000000, 0x0000000),
    (0x1954248, 0x1953248),
    (0x266CA30, 0x266AA30),
    (0x26F3E00, 0x26F0E00),
)


def va_to_file(va: int) -> int:
    for vbase, fbase in reversed(LOAD_SEGMENTS):
        if va >= vbase:
            return va - vbase + fbase
    return va


def file_to_va(fo: int) -> int:
    for vbase, fbase in reversed(LOAD_SEGMENTS):
        if fo >= fbase:
            return fo - fbase + vbase
    return fo


def rel32(from_va: int, to_va: int) -> bytes:
    return struct.pack("<i", to_va - (from_va + 5))


def jmp_rel32(at_va: int, to_va: int) -> bytes:
    return b"\xe9" + rel32(at_va, to_va)


def call_rel32(at_va: int, to_va: int) -> bytes:
    return b"\xe8" + rel32(at_va, to_va)


# P1–P6 from HANDOFF.md (verified offsets).
PATCHES: dict[str, tuple[int, bytes]] = {
    "P1": (0x196E300, bytes([0x01])),
    "P2": (0x19BEA36, bytes([0x90, 0x90])),
    "P3": (0x1AB64D1, bytes([0x90, 0x90])),
    "P4": (0x1AA629C, bytes([0x90] * 6)),
    "P5": (0x1B38A60, bytes([0x90] * 10)),
    "P6": (0x1B38A95, bytes([0x48, 0x31, 0xF6, 0xE9, 0x73, 0xFE, 0xFF, 0xFF, 0x90, 0x90])),
    # P8 (optional): force blitRect slow path — disabled; regressed fill rendering.
    # "P8": (0x1B2222F, bytes([0xEB, 0x38])),
    # P10: SkBitmapDevice::drawPaint — always take post-onAccessPixels path.
    "P10": (0x1A8AE87, bytes([0xEB, 0x10])),
}

# P7/P9: wire Draw.fPixels from SkBitmapDevice before skcpu::Draw::drawPaint.
P9_HOOK_VA = 0x1A8BEB8
P9_RESUME_VA = 0x1A8BEC5
P9_CAVE_FILE = 0x1F4438C
P9_CAVE_VA = file_to_va(P9_CAVE_FILE)
P9_PROLOGUE_FILE = va_to_file(0x1A8BE5F)
P9_PROLOGUE_ORIG = bytes([0x48, 0x8D, 0x7C, 0x24, 0x08, 0xE8, 0xA7, 0x8C, 0x02, 0x00])
P9_EPILOGUE_FILE = va_to_file(0x1A8BEC5)
P9_EPILOGUE_ORIG = bytes([
    0x48, 0x8D, 0x05, 0x9C, 0x4F, 0xBF, 0x00,  # lea rax, [rip+0xbf4f9c]
    0x48, 0x83, 0xC0, 0x10,                   # add rax, 0x10
    0x48, 0x89, 0x44, 0x24, 0x08,             # mov [rsp+8], rax
])


def build_p9_cave() -> bytes:
    """Copy device.fPixels/fRowBytes into Draw.fDst, then call skcpu::Draw::drawPaint.

    The destination pixel pointer and row-bytes live at the SAME device offsets
    that SkBitmapDevice::onAccessPixels reads in this engine build:
        fPixels   = *(void**)(device + 0x140)
        fRowBytes = *(size_t*)(device + 0x148)
    (Verified by disassembling onAccessPixels: it calls
     SkPixmap::reset(info, [device+0x140], [device+0x148]).)

    The older cave used [device+0x180]+[..+0x18], which in this build is a
    clip/matrix pointer (SkDraw.fRC at [rsp+0x48]) — NOT the pixels — so the
    blitter wrote to the wrong address and every fill came out zero.
    """
    cave: list[bytes] = []
    va = P9_CAVE_VA

    def emit(data: bytes) -> None:
        nonlocal va
        cave.append(data)
        va += len(data)

    emit(b"\x49\x8b\x86\x40\x01\x00\x00")  # mov rax, [r14+0x140]  ; fPixels
    emit(b"\x48\x89\x44\x24\x10")           # mov [rsp+0x10], rax  ; Draw.fDst.fPixels
    emit(b"\x49\x8b\x86\x48\x01\x00\x00")  # mov rax, [r14+0x148]  ; fRowBytes (64-bit)
    emit(b"\x48\x89\x44\x24\x18")           # mov [rsp+0x18], rax  ; Draw.fDst.fRowBytes
    emit(b"\x48\x8d\x7c\x24\x08")           # lea rdi, [rsp+8]
    emit(b"\x48\x89\xde")                   # mov rsi, rbx
    call_at = va
    emit(call_rel32(call_at, 0x1AB74B0))
    emit(jmp_rel32(va, P9_RESUME_VA))
    blob = b"".join(cave)
    if len(blob) > 52:
        raise RuntimeError(f"P9 cave too large: {len(blob)} bytes")
    return blob


def p9_hook_bytes() -> bytes:
    hook = jmp_rel32(P9_HOOK_VA, P9_CAVE_VA)
    assert len(hook) == 5
    return hook + b"\x90" * 8


PATCHES["P9"] = (va_to_file(P9_HOOK_VA), p9_hook_bytes())
PATCHES["P9_CAVE"] = (P9_CAVE_FILE, build_p9_cave())
PATCHES["P9_PROLOGUE"] = (P9_PROLOGUE_FILE, P9_PROLOGUE_ORIG)
PATCHES["P9_EPILOGUE"] = (P9_EPILOGUE_FILE, P9_EPILOGUE_ORIG)

# Legacy aliases kept so older docs/commands still resolve.
PATCHES["P7"] = PATCHES["P9"]
PATCHES["P7_CAVE"] = PATCHES["P9_CAVE"]
PATCHES["P7_PROLOGUE"] = PATCHES["P9_PROLOGUE"]
PATCHES["P7_EPILOGUE"] = PATCHES["P9_EPILOGUE"]


def patch_kernel_blob(path: Path) -> None:
    """Null the 10-char SDK hash in a kernel_blob.bin (Dill header offset 8)."""
    data = path.read_bytes()
    if len(data) < 18:
        raise ValueError(f"kernel blob too short: {path}")
    if data[0:4] != b"\x90\xab\xcd\xef":
        print(f"warning: unexpected dill magic in {path}: {data[0:4].hex()}")
    current = data[8:18]
    try:
        current_str = current.decode("ascii")
    except UnicodeDecodeError:
        current_str = current.hex()
    print(f"kernel blob SDK hash in {path}: {current_str}")
    with path.open("r+b") as f:
        f.seek(8)
        f.write(b"0000000000")
    print(f"patched -> 0000000000")

def verify(data: bytes, name: str | None = None) -> list[str]:
    errors: list[str] = []
    items = PATCHES.items() if name is None else [(name, PATCHES[name])]
    for patch_name, (offset, expected) in items:
        actual = data[offset : offset + len(expected)]
        if actual != expected:
            errors.append(
                f"{patch_name} @ 0x{offset:x}: expected {expected.hex()} got {actual.hex()}"
            )
    return errors


def apply(data: bytearray, names: list[str]) -> None:
    for name in names:
        offset, patch = PATCHES[name]
        data[offset : offset + len(patch)] = patch


def main() -> int:
    parser = argparse.ArgumentParser(description="Patch Flutter engine / kernel blobs for OSCortex")
    parser.add_argument("--engine", type=Path, default=DEFAULT_ENGINE)
    parser.add_argument("--kernel-blob", type=Path, help="Patch SDK hash in kernel_blob.bin")
    parser.add_argument("--verify", action="store_true", help="Verify engine patches")
    parser.add_argument("--apply", nargs="*", metavar="PATCH", help="Apply patch names (default: P9)")
    parser.add_argument("--apply-all", action="store_true", help="Apply P1–P6, P9, P10")
    parser.add_argument("--backup", action="store_true", help="Create .bak before applying engine patches")
    args = parser.parse_args()

    if args.kernel_blob is not None:
        if not args.kernel_blob.exists():
            print(f"kernel blob not found: {args.kernel_blob}", file=sys.stderr)
            return 1
        patch_kernel_blob(args.kernel_blob)
        return 0

    engine: Path = args.engine
    if not engine.exists():
        print(f"engine not found: {engine}", file=sys.stderr)
        return 1

    data = bytearray(engine.read_bytes())

    if args.verify:
        names = (
            ["P1", "P2", "P3", "P4", "P5", "P6", "P10", "P9", "P9_CAVE", "P9_PROLOGUE", "P9_EPILOGUE"]
            if args.apply_all
            else None
        )
        if names:
            errors = []
            for n in names:
                errors.extend(verify(data, n))
        else:
            errors = verify(data, "P1")
            for n in ["P2", "P3", "P4", "P5", "P6", "P10", "P9", "P9_CAVE", "P9_PROLOGUE", "P9_EPILOGUE"]:
                errors.extend(verify(data, n))
        if errors:
            for err in errors:
                print(f"MISMATCH: {err}")
            return 1
        print(f"All requested patches OK in {engine}")
        return 0

    if args.apply is not None and len(args.apply) == 0 and not args.apply_all:
        args.apply = ["P9"]

    to_apply = (
        ["P1", "P2", "P3", "P4", "P5", "P6", "P10", "P9", "P9_CAVE", "P9_PROLOGUE", "P9_EPILOGUE"]
        if args.apply_all
        else (args.apply or [])
    )
    if "P9" in to_apply and "P9_CAVE" not in to_apply:
        to_apply.extend(["P9_CAVE", "P9_PROLOGUE", "P9_EPILOGUE"])
    if "P7" in to_apply:
        to_apply.extend(["P7_CAVE", "P7_PROLOGUE", "P7_EPILOGUE"])
    # Preserve order, drop duplicates.
    seen: set[str] = set()
    to_apply = [n for n in to_apply if not (n in seen or seen.add(n))]
    unknown = [n for n in to_apply if n not in PATCHES]
    if unknown:
        print(f"unknown patches: {unknown}", file=sys.stderr)
        return 1

    if args.backup:
        bak = engine.with_suffix(engine.suffix + ".bak")
        shutil.copy2(engine, bak)
        print(f"backup -> {bak}")

    apply(data, to_apply)
    engine.write_bytes(data)
    print(f"applied {', '.join(to_apply)} -> {engine}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
