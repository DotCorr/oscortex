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


def get_sdk_hash_from_elf(elf_path: Path) -> bytes | None:
    import subprocess
    try:
        out = subprocess.check_output(["nm", "-D", str(elf_path)], text=True)
        for line in out.splitlines():
            if "_kDartVmSnapshotData" in line:
                parts = line.split()
                if len(parts) >= 3:
                    val = int(parts[0], 16)
                    data = elf_path.read_bytes()
                    e_phoff = struct.unpack("<Q", data[0x20:0x28])[0]
                    e_phnum = struct.unpack("<H", data[0x38:0x3a])[0]
                    e_phentsize = struct.unpack("<H", data[0x36:0x38])[0]
                    
                    file_offset = None
                    for i in range(e_phnum):
                        ph_offset = e_phoff + i * e_phentsize
                        p_type = struct.unpack("<I", data[ph_offset : ph_offset + 4])[0]
                        if p_type == 1: # PT_LOAD
                            p_offset = struct.unpack("<Q", data[ph_offset + 8 : ph_offset + 16])[0]
                            p_vaddr = struct.unpack("<Q", data[ph_offset + 16 : ph_offset + 24])[0]
                            p_filesz = struct.unpack("<Q", data[ph_offset + 32 : ph_offset + 40])[0]
                            if val >= p_vaddr and val < p_vaddr + p_filesz:
                                file_offset = p_offset + (val - p_vaddr)
                                break
                    
                    if file_offset is not None:
                        hash_bytes = data[file_offset + 20 : file_offset + 30]
                        return hash_bytes
    except Exception as e:
        print(f"Error extracting SDK hash: {e}")
    return None


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
    # P_SDK_HASH, P_SDK_HASH_2, P_SDK_HASH_3: expected SDK hash offsets in pristine engine.
    "P_SDK_HASH": (0x188ced, b"1a420a3f9a"),
    "P_SDK_HASH_2": (0x3bd638, b"1a420a3f9a"),
    "P_SDK_HASH_3": (0xd81028, b"1a420a3f9a"),
    # P_RUNS_AOT: patch runs_aot_compiled_dart_code to return true.
    "P_RUNS_AOT": (0x195f260, b"\xb0\x01\xc3\x90\x90"),
    # P_JIT_AOT_CHECK: bypass "JIT runtime cannot run a precompiled snapshot" panic
    "P_JIT_AOT_CHECK": (0x22b181b, bytes([0x90, 0x90])),
    # P_SNAPSHOT_FEATURES_CHECK: bypass snapshot/VM features mismatch panic (e.g. macos vs linux)
    "P_SNAPSHOT_FEATURES_CHECK": (0x2290fe6, bytes([0xeb, 0x73])),
    # P_ALLOW_ALL_DART_FLAGS: bypass switches.cc allowed Dart flags whitelist check
    "P_ALLOW_ALL_DART_FLAGS": (0x21c3f40, bytes([0x74, 0xac])),
}

# P7/P9: wire Draw.fPixels from SkBitmapDevice before skcpu::Draw::drawPaint.
P9_HOOK_VA = 0x1A8BEB8
P9_RESUME_VA = 0x1A8BEC5
P9_CAVE_FILE = 0x1F4438C
P9_CAVE_VA = file_to_va(P9_CAVE_FILE)

# Diagnostic log stub: prints "FP" + 16 hex digits of device[0x140] (fPixels)
# via the SYS_WRITE(fd=1) syscall, then returns. Placed further into the
# (unused-in-software-engine) ChaCha20 region. Set DIAG_LOG=False to disable.
DIAG_LOG = False
# When set, the P9 cave manually memsets the whole device buffer to a sentinel
# colour before calling drawPaint. Used to prove the present/framebuffer path
# works independently of Skia's (broken) software blitter.
DIAG_FILL = False
DIAG_FILL_COLOR = 0xFFFF00FF
# When set, skip the real skcpu::Draw::drawPaint call (only the sentinel fill
# runs). Used to prove drawPaint is actively zero-filling the surface.
DIAG_SKIP_DRAWPAINT = False
LOG_CAVE_FILE = 0x1F44400
LOG_CAVE_VA = file_to_va(LOG_CAVE_FILE)


def build_log_stub() -> bytes:
    """Print three labelled 64-bit device fields via SYS_WRITE(fd=1):
        FP = device[0x140]  (fPixels)
        CT = device[0x158]  (SkColorInfo.fColorType|fAlphaType)
        WH = device[0x160]  (fWidth | fHeight<<32)
    """
    def fmt_rax(label: bytes) -> list[int]:
        """Format the 64-bit value in rax as 'XX<16 hex>\\n' and SYS_WRITE it."""
        l0, l1 = label[0], label[1]
        return [
            0x48, 0x83, 0xec, 0x20,            # sub rsp, 0x20
            0xc6, 0x04, 0x24, l0,              # mov byte [rsp], label[0]
            0xc6, 0x44, 0x24, 0x01, l1,        # mov byte [rsp+1], label[1]
            0x48, 0x8d, 0x7c, 0x24, 0x02,      # lea rdi, [rsp+2]
            0xb9, 0x10, 0x00, 0x00, 0x00,      # mov ecx, 16
            # .loop:
            0x48, 0xc1, 0xc0, 0x04,            # rol rax, 4
            0x48, 0x89, 0xc3,                  # mov rbx, rax
            0x83, 0xe3, 0x0f,                  # and ebx, 0xf
            0x80, 0xc3, 0x30,                  # add bl, 0x30
            0x80, 0xfb, 0x3a,                  # cmp bl, 0x3a
            0x72, 0x03,                        # jb +3
            0x80, 0xc3, 0x27,                  # add bl, 0x27
            0x88, 0x1f,                        # mov [rdi], bl
            0x48, 0xff, 0xc7,                  # inc rdi
            0xff, 0xc9,                        # dec ecx
            0x75, 0xe2,                        # jnz .loop
            0xc6, 0x07, 0x0a,                  # mov byte [rdi], 0x0a
            0x48, 0xff, 0xc7,                  # inc rdi
            0x48, 0x89, 0xfa,                  # mov rdx, rdi
            0x48, 0x29, 0xe2,                  # sub rdx, rsp
            0x48, 0x89, 0xe6,                  # mov rsi, rsp
            0xbf, 0x01, 0x00, 0x00, 0x00,      # mov edi, 1
            0xb8, 0x01, 0x00, 0x00, 0x00,      # mov eax, 1 (SYS_WRITE)
            0x0f, 0x05,                        # syscall
            0x48, 0x83, 0xc4, 0x20,            # add rsp, 0x20
        ]

    def load_field(off: int) -> list[int]:
        return [0x49, 0x8b, 0x86, off & 0xff, (off >> 8) & 0xff, 0x00, 0x00]  # mov rax,[r14+off]

    body: list[int] = []
    body += [0x50, 0x53, 0x51, 0x52, 0x56, 0x57, 0x41, 0x50, 0x41, 0x53]  # push regs
    body += [0x49, 0x89, 0xd8]                       # mov r8, rbx  (save paint ptr; fmt_rax clobbers rbx)
    # CPU self-test: SS = bits of (float)255 via cvtsi2ss (expect 0x437F0000).
    #                DQ = bits of (float)255 via cvtdq2ps  (expect 0x437F0000).
    body += [
        0xb9, 0xff, 0x00, 0x00, 0x00,        # mov ecx, 255
        0xf3, 0x0f, 0x2a, 0xc1,              # cvtsi2ss xmm0, ecx
        0x66, 0x0f, 0x7e, 0xc0,              # movd eax, xmm0
    ]
    body += fmt_rax(b"SS")
    body += [
        0xb9, 0xff, 0x00, 0x00, 0x00,        # mov ecx, 255
        0x66, 0x0f, 0x6e, 0xc1,              # movd xmm0, ecx
        0x0f, 0x5b, 0xc0,                    # cvtdq2ps xmm0, xmm0
        0x66, 0x0f, 0x7e, 0xc0,              # movd eax, xmm0
    ]
    body += fmt_rax(b"DQ")
    # Paint SkColor4f lives at SkPaint+0x30 (verified via SkPaint::setColor:
    # 'movups [rbx+0x30], xmm0').  PA=[r8+0x30] (R|G), PB=[r8+0x38] (B|A).
    body += [0x49, 0x8b, 0x40, 0x30] + fmt_rax(b"PA")  # mov rax,[r8+0x30]  R|G floats
    body += [0x49, 0x8b, 0x40, 0x38] + fmt_rax(b"PB")  # mov rax,[r8+0x38]  B|A floats
    body += load_field(0x140) + fmt_rax(b"FP")
    body += load_field(0x158) + fmt_rax(b"CT")
    body += load_field(0x160) + fmt_rax(b"WH")
    # DC = device[0x180]
    body += load_field(0x180) + fmt_rax(b"DC")
    # clip = device[0x180] + (int)device[0x180][0x18];  CB = [clip+0x8] (right|bottom)
    body += [
        0x49, 0x8b, 0x86, 0x80, 0x01, 0x00, 0x00,  # mov rax,[r14+0x180]
        0x48, 0x63, 0x48, 0x18,                    # movsxd rcx,[rax+0x18]
        0x48, 0x01, 0xc8,                          # add rax,rcx       (rax = clip)
        0x48, 0x8b, 0x40, 0x08,                    # mov rax,[rax+0x8] (right|bottom)
    ]
    body += fmt_rax(b"CB")
    # RB = *(u32*)device[0x140]  (read back first pixel of the surface buffer)
    body += [
        0x49, 0x8b, 0x86, 0x40, 0x01, 0x00, 0x00,  # mov rax,[r14+0x140]
        0x8b, 0x00,                                # mov eax,[rax]
    ]
    body += fmt_rax(b"RB")
    body += [0x41, 0x5b, 0x41, 0x58, 0x5f, 0x5e, 0x5a, 0x59, 0x5b, 0x58]  # pop regs
    body += [0xc3]                                                         # ret
    return bytes(body)
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
    emit(b"\x49\x8b\x96\x48\x01\x00\x00")  # mov rdx, [r14+0x148]  ; fRowBytes (64-bit)
    emit(b"\x48\x89\x54\x24\x18")           # mov [rsp+0x18], rdx  ; Draw.fDst.fRowBytes
    if DIAG_FILL:
        # rcx = height ([r14+0x164]) * rowBytes ([r14+0x148]); /4 => dword count
        emit(b"\x45\x8b\x96\x64\x01\x00\x00")      # mov r10d, [r14+0x164]  ; height
        emit(b"\x4d\x0f\xaf\x96\x48\x01\x00\x00")  # imul r10, [r14+0x148]  ; *rowBytes
        emit(b"\x49\xc1\xea\x02")                   # shr r10, 2             ; dword count
        emit(b"\x49\x89\xc1")                       # mov r9, rax           ; cursor
        emit(b"\xba" + struct.pack("<I", DIAG_FILL_COLOR))  # mov edx, color
        # .fill:
        emit(b"\x41\x89\x11")                       # mov [r9], edx
        emit(b"\x49\x83\xc1\x04")                   # add r9, 4
        emit(b"\x49\xff\xca")                       # dec r10
        emit(b"\x75\xf4")                           # jnz .fill
    if DIAG_LOG:
        emit(call_rel32(va, LOG_CAVE_VA))  # log stub (FP/CT/WH/DC/CB/RB)
    if not DIAG_SKIP_DRAWPAINT:
        emit(b"\x48\x8d\x7c\x24\x08")           # lea rdi, [rsp+8]
        emit(b"\x48\x89\xde")                   # mov rsi, rbx
        call_at = va
        emit(call_rel32(call_at, 0x1AB74B0))
    emit(jmp_rel32(va, P9_RESUME_VA))
    blob = b"".join(cave)
    cave_budget = P9_CAVE_VA  # cave region runs up to LOG_CAVE_VA
    if P9_CAVE_VA + len(blob) > LOG_CAVE_VA:
        raise RuntimeError(f"P9 cave overruns log stub: {len(blob)} bytes")
    return blob


def p9_hook_bytes() -> bytes:
    hook = jmp_rel32(P9_HOOK_VA, P9_CAVE_VA)
    assert len(hook) == 5
    return hook + b"\x90" * 8


PATCHES["P9"] = (va_to_file(P9_HOOK_VA), p9_hook_bytes())
PATCHES["P9_CAVE"] = (P9_CAVE_FILE, build_p9_cave())
if DIAG_LOG:
    PATCHES["LOG_CAVE"] = (LOG_CAVE_FILE, build_log_stub())
PATCHES["P9_PROLOGUE"] = (P9_PROLOGUE_FILE, P9_PROLOGUE_ORIG)
PATCHES["P9_EPILOGUE"] = (P9_EPILOGUE_FILE, P9_EPILOGUE_ORIG)

# Legacy aliases kept so older docs/commands still resolve.
PATCHES["P7"] = PATCHES["P9"]
PATCHES["P7_CAVE"] = PATCHES["P9_CAVE"]
PATCHES["P7_PROLOGUE"] = PATCHES["P9_PROLOGUE"]
PATCHES["P7_EPILOGUE"] = PATCHES["P9_EPILOGUE"]


def patch_kernel_blob(path: Path) -> None:
    """Patch the 10-char SDK hash in a kernel_blob.bin (Dill header offset 8)."""
    libapp_path = ROOT / "initramfs/system/flutter/libapp.so"
    extracted_hash = get_sdk_hash_from_elf(libapp_path) if libapp_path.exists() else None
    target_hash = extracted_hash or b"78da37fed6"
    
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
        f.write(target_hash)
    print(f"patched -> {target_hash.decode('ascii')}")

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

    # Extract dynamic SDK hash from libapp.so to update P_SDK_HASH, P_SDK_HASH_2, P_SDK_HASH_3
    libapp_path = ROOT / "initramfs/system/flutter/libapp.so"
    extracted_hash = get_sdk_hash_from_elf(libapp_path) if libapp_path.exists() else None
    if extracted_hash:
        print(f"Extracted dynamic SDK hash from libapp.so: {extracted_hash.decode('ascii')}")
        PATCHES["P_SDK_HASH"] = (0x188ced, extracted_hash)
        PATCHES["P_SDK_HASH_2"] = (0x3bd638, extracted_hash)
        PATCHES["P_SDK_HASH_3"] = (0xd81028, extracted_hash)
    else:
        print("Using fallback SDK hash 78da37fed6")
        PATCHES["P_SDK_HASH"] = (0x188ced, b"78da37fed6")
        PATCHES["P_SDK_HASH_2"] = (0x3bd638, b"78da37fed6")
        PATCHES["P_SDK_HASH_3"] = (0xd81028, b"78da37fed6")

    data = bytearray(engine.read_bytes())

    if args.verify:
        names = (
            ["P1", "P2", "P3", "P4", "P5", "P6", "P10", "P9", "P9_CAVE", "P9_PROLOGUE", "P9_EPILOGUE", "P_SDK_HASH", "P_SDK_HASH_2", "P_SDK_HASH_3", "P_RUNS_AOT", "P_JIT_AOT_CHECK", "P_SNAPSHOT_FEATURES_CHECK", "P_ALLOW_ALL_DART_FLAGS"]
            if args.apply_all
            else None
        )
        if names:
            errors = []
            for n in names:
                errors.extend(verify(data, n))
        else:
            errors = verify(data, "P1")
            for n in ["P2", "P3", "P4", "P5", "P6", "P10", "P9", "P9_CAVE", "P9_PROLOGUE", "P9_EPILOGUE", "P_SDK_HASH", "P_SDK_HASH_2", "P_SDK_HASH_3", "P_RUNS_AOT", "P_JIT_AOT_CHECK", "P_SNAPSHOT_FEATURES_CHECK", "P_ALLOW_ALL_DART_FLAGS"]:
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
        ["P1", "P2", "P3", "P4", "P5", "P6", "P10", "P9", "P9_CAVE", "P9_PROLOGUE", "P9_EPILOGUE", "P_SDK_HASH", "P_SDK_HASH_2", "P_SDK_HASH_3", "P_RUNS_AOT", "P_JIT_AOT_CHECK", "P_SNAPSHOT_FEATURES_CHECK", "P_ALLOW_ALL_DART_FLAGS"]
        if args.apply_all
        else (args.apply or [])
    )
    if "P9" in to_apply and "P9_CAVE" not in to_apply:
        to_apply.extend(["P9_CAVE", "P9_PROLOGUE", "P9_EPILOGUE"])
    if DIAG_LOG and "LOG_CAVE" in PATCHES and "LOG_CAVE" not in to_apply:
        to_apply.append("LOG_CAVE")
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
