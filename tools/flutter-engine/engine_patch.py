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
            if "kDartVmSnapshotData" in line:
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
    "P_SDK_HASH": (0x188ced, b"78da37fed6"),
    "P_SDK_HASH_2": (0x3bd638, b"78da37fed6"),
    "P_SDK_HASH_3": (0xd81028, b"78da37fed6"),
    # P_RUNS_AOT: patch runs_aot_compiled_dart_code to return true.
    "P_RUNS_AOT": (0x195f260, b"\xb0\x01\xc3\x90\x90"),
    # P_JIT_AOT_CHECK: bypass "JIT runtime cannot run a precompiled snapshot" panic
    "P_JIT_AOT_CHECK": (0x22b181b, bytes([0x90, 0x90])),
    # P_SNAPSHOT_FEATURES_CHECK: bypass snapshot/VM features mismatch panic (e.g. macos vs linux)
    "P_SNAPSHOT_FEATURES_CHECK": (0x2290f30, bytes([0x48, 0x31, 0xc0, 0xc3, 0x90, 0x90, 0x90])),
    # P_ALLOW_ALL_DART_FLAGS: bypass switches.cc allowed Dart flags whitelist check
    # We change the conditional jump (je success_path) at 0x21c3f2f to an unconditional
    # jump (jmp success_path) so all Dart flags are accepted.
    "P_ALLOW_ALL_DART_FLAGS": (0x21c3f2f, bytes([0xe9, 0xba, 0x00, 0x00, 0x00, 0x90])),
    # P_FINISH_INIT_HOOK and P_FINISH_INIT_CAVE: bypass NULL pointer dereference in
    # dart::Object::FinishInit(dart::IsolateGroup*). If rax (type testing stub code) is 0,
    # jump directly to the end of the stub initialization to avoid segfaulting.
    "P_FINISH_INIT_HOOK": (0x23147e0, bytes([0xe9, 0x1b, 0xfe, 0xc2, 0xff])),
    "P_FINISH_INIT_CAVE": (0x1F44600, bytes([0x48, 0x85, 0xc0, 0x74, 0x0a, 0x48, 0x39, 0xf8, 0x74, 0x05, 0xe9, 0xda, 0x01, 0x3d, 0x00, 0xe9, 0xd1, 0x01, 0x3d, 0x00])),
    "P_FINISH_INIT_HOOK_2": (0x2314861, bytes([0xe9, 0xba, 0xfd, 0xc2, 0xff])),
    "P_FINISH_INIT_CAVE_2": (0x1F44620, bytes([0x48, 0x85, 0xc0, 0x74, 0x0a, 0x48, 0x39, 0xf8, 0x74, 0x05, 0xe9, 0x3b, 0x02, 0x3d, 0x00, 0xe9, 0x32, 0x02, 0x3d, 0x00])),
    "P_INIT_TTS_HOOK": (0x23148cf, bytes([0xe9, 0x6c, 0xfd, 0xc2, 0xff])),
    "P_INIT_TTS_CAVE": (0x1F44640, bytes([0x48, 0x85, 0xd2, 0x74, 0x0a, 0x4c, 0x39, 0xc2, 0x74, 0x05, 0xe9, 0x89, 0x02, 0x3d, 0x00, 0xe9, 0x80, 0x02, 0x3d, 0x00])),
    "P_BYPASS_TTS_INIT_IN_VM_CONSTANTS_PART1": (0x23dac7e, bytes([0xe9, 0x7d, 0x9a, 0xb6, 0xff, 0x90, 0x90])),
    "P_BYPASS_TTS_INIT_IN_VM_CONSTANTS_PART2": (0x23db253, bytes([0xe9, 0x67, 0x00, 0x00, 0x00, 0x90, 0x90])),
    "P_VM_CONSTANTS_CAVE": (0x1F44700, bytes([
        0x4c, 0x8d, 0x0d, 0x63, 0x00, 0x00, 0x00, 0x4d, 0x31, 0xc0, 0x49, 0x83, 0xf8, 0x12, 0x74, 0x55,
        0x4f, 0x0f, 0xb7, 0x14, 0x41, 0x4b, 0x8b, 0x0c, 0x17, 0x48, 0x81, 0xf9, 0x00, 0x10, 0x00, 0x00,
        0x72, 0x36, 0x48, 0x8b, 0x51, 0x08, 0x48, 0x81, 0xfa, 0x00, 0x10, 0x00, 0x00, 0x72, 0x29, 0x48,
        0x8b, 0x4a, 0x2f, 0x48, 0x39, 0xc1, 0x75, 0x04, 0x48, 0x8b, 0x4a, 0x07, 0x49, 0x83, 0xf8, 0x10,
        0x75, 0x09, 0x48, 0x39, 0xc1, 0x74, 0x04, 0x48, 0x83, 0xc1, 0x05, 0x4a, 0x89, 0x8c, 0xc3, 0xf8,
        0x01, 0x00, 0x00, 0x49, 0xff, 0xc0, 0xeb, 0xb2, 0x4a, 0x89, 0x84, 0xc3, 0xf8, 0x01, 0x00, 0x00,
        0x49, 0xff, 0xc0, 0xeb, 0xa5, 0xe9, 0x49, 0x6a, 0x49, 0x00,
        # Table of IsolateGroup/Isolate offsets to check (now starts with 0xa0):
        0xa0, 0x00,
        0xe0, 0x00, 0x40, 0x06, 0xa0, 0x03, 0xc0, 0x03, 0xa0, 0x04, 0xc0, 0x04, 0xe0, 0x04, 0xe0, 0x0f,
        0xc0, 0x0f, 0x60, 0x08, 0xe0, 0x07, 0x60, 0x07, 0xc0, 0x08, 0xa0, 0x10, 0x20, 0x00, 0x00, 0x0d,
        0xa0, 0x06,
    ])),
    "P_BYPASS_FINALIZE_CLASS_NAMES": (0x2314b3e, bytes([0xe9, 0xba, 0x04, 0x00, 0x00, 0x90, 0x90])),
    "P_BYPASS_BASE_OBJECTS_CHECK": (0x2291627, bytes([0xeb])),
    "P_BYPASS_BASE_OBJECTS_COMPARE": (0x22914aa, bytes([0x90] * 6)),
    "P_IS_PRECOMPILED_RUNTIME": (0x2668190, bytes([0xb0, 0x01, 0xc3])),
    "P_FLAG_PRECOMPILED_MODE_DEFAULT": (0x22ce1a8, bytes([0xb2, 0x01])),
    "P_FLAG_PRECOMPILED_MODE_INITIAL": (0x22ce1af, bytes([0xc6, 0x05, 0xeb, 0x02, 0x45, 0x00, 0x01])),
    "P_BYPASS_VM_SERVICE_TASK": (0x23ca510, bytes([0xc3, 0x90, 0x90, 0x90, 0x90])),
    "P_BYPASS_SERVICE_ISOLATE_RUN": (0x23c9eb0, bytes([0xc3, 0x90, 0x90, 0x90, 0x90])),
    "P_READFILL_OBJPOOL_0": (0x2298f74, bytes.fromhex("4d89ac2497000000") + b"\x90" * 12),
    "P_READFILL_OBJPOOL": (0x2299024, bytes.fromhex("4d896c2427") + b"\x90" * 12),
    "P_READFILL_OBJPOOL_2": (0x22990a4, bytes.fromhex("4d896c2437") + b"\x90" * 12),
    "P_READFILL_OBJPOOL_3": (0x2299124, bytes.fromhex("4d896c243f") + b"\x90" * 12),
    "P_READFILL_OBJPOOL_4": (0x22991a4, bytes.fromhex("4d896c2447") + b"\x90" * 12),
    "P_READFILL_OBJPOOL_5": (0x2299224, bytes.fromhex("4d896c244f") + b"\x90" * 12),
    "P_READFILL_STACKMAPS": (0x22992a4, bytes.fromhex("4d896c2457") + b"\x90" * 12),
    "P_READFILL_OBJPOOL_7": (0x2299324, bytes.fromhex("4d896c245f") + b"\x90" * 12),
    "P_READFILL_OBJPOOL_8": (0x22993a4, bytes.fromhex("4d896c2467") + b"\x90" * 12),
    "P_READFILL_OBJPOOL_9": (0x2299424, bytes.fromhex("4d896c2477") + b"\x90" * 12),
    "P_READFILL_OBJPOOL_10": (0x22994a4, bytes.fromhex("4d896c247f") + b"\x90" * 12),
    "P_READFILL_OBJPOOL_11": (0x2299524, bytes.fromhex("4d89ac2487000000") + b"\x90" * 12),
    "P_READ_INSTRUCTIONS": (0x2291070, bytes.fromhex("e98b39cbff") + b"\x90" * 491),
    "P_DEBUG_CAVE": (0x1F44A00, bytes.fromhex("4156535541574d89f74889f34989fe48b8bbbbbbbbbbbbbbbbe8ff0000004c89f8e8f70000004889d8e8ef000000498b4648e8e6000000498b4e48488b01e8da000000498b7648e83801000049897648e8c80000004889c6498b7e58e8ffa338004889432f498b4648e8af000000498b4e48488b01e8a3000000498b7648e86c01000049897648e8910000008983ab000000498b4648e882000000498b4e48488b01e876000000498b7648e8d400000049897648e8640000004889c6498b7e58e89ba338004889436f4889c5e84c000000498b4648e843000000498b4e48488b01e837000000498b7648e80001000049897648e8250000005048b8eeeeeeeeeeeeeeeee815000000584889df4889ee4889c2415f5d5b415ee9431440004981ffc00400007205e801000000c357565251415350534883ec204889e748c7c11000000048c1c00489c383e30f80c33080fb39760380c307881f48ffc748ffc975e2c6070ab801000000bf010000004889e6ba110000000f054883c4205b58415b595a5e5fc30fb60648ffc684c078410fb60e48ffc689cac1e20709d084c978380fb60e48ffc689cac1e20e09d084c9782f0fb60e48ffc689cac1e21509d084c978260fb60e48ffc689cac1e21c09d0c32dc00000004898c32d006000004898c32d000030004898c32d000000184898c30fb60648ffc684c078410fb60e48ffc689cac1e20709d084c978380fb60e48ffc689cac1e20e09d084c9782f0fb60e48ffc689cac1e21509d084c978260fb60e48ffc689cac1e21c09d0c32d800000004898c32d004000004898c32d000020004898c32d000000104898c3")),
    "P_CLUSTER_TRACE_HOOK": (0x2291551, bytes.fromhex("e8aa4acbff90")),
    "P_CLUSTER_TRACE_CAVE": (0x1F46000, bytes.fromhex("5756525141504151415241534154415541564157535548b8cccccccccccccccce8850000004c89e8e87d0000004889f8e875000000498b4648e86c0000005d5b415f415e415d415c415b415a41594158595a5e5f488b074883ec08ff50184883c40857565251415041514152415341544155415641575355498b4648e82900000048b8dddddddddddddddde81a0000005d5b415f415e415d415c415b415a41594158595a5e5f49ffc5c357565251415350534883ec204889e748c7c11000000048c1c00489c383e30f80c33080fb39760380c307881f48ffc748ffc975e2c6070ab801000000bf010000004889e6ba110000000f054883c4205b58415b595a5e5fc3")),
    "P_READFILL_LOOP_CLAMP_HOOK": (va_to_file(0x2299f97), bytes.fromhex("e964b9caff90909090")),
    "P_READFILL_LOOP_CLAMP_CAVE": (0x1F44900, bytes([
        0x50, 0x57, 0x56, 0x52, 0x51, 0x41, 0x50, 0x41, 0x51, 0x41, 0x53, # push regs
        0x48, 0xc7, 0xc0, 0x99, 0x99, 0x00, 0x00,                         # mov rax, 0x9999
        0x48, 0x89, 0xdf,                                                 # mov rdi, rbx
        0x0f, 0x05,                                                       # syscall
        0x41, 0x5b, 0x41, 0x59, 0x41, 0x58, 0x59, 0x5a, 0x5e, 0x5f, 0x58, # pop regs
        0x48, 0x89, 0xd8,                                                 # mov rax, rbx
        0x48, 0x83, 0xfb, 0x64,                                           # cmp rbx, 100
        0x7c, 0x04,                                                       # jl .no_clamp
        0x48, 0x83, 0xe8, 0x04,                                           # sub rax, 4
        # .no_clamp:
        0x49, 0x39, 0xc6,                                                 # cmp r14, rax
        0x0f, 0x84, 0x70, 0x4c, 0x35, 0x00,                               # je 0x229a5a8
        0xe9, 0x63, 0x46, 0x35, 0x00                                      # jmp 0x2299fa0
    ])),
    # P_READFILL_LOOP_CLAMP_HOOK / CAVE intentionally omitted from all_patches (see comment there).
}

# ── All-cluster object pool lookup mass patch ─────────────────────────────────
# Replaces ALL `movq 0x417(%base,%idx*8), %dst` pool lookup variants across ALL
# deserialization cluster ReadFill/ReadFromTo functions with xor dst_reg,dst_reg
# (zero the destination register, safe no-op for deser stub purposes).
# Covers 7 register-combination forms totalling 113 instructions.
_POOL_DISP = bytes([0x17, 0x04, 0x00, 0x00])
_POOL_PREFIX = bytes([0x48, 0x8B])
# Map (ModRM, SIB) -> 8-byte zero+nop replacement
_POOL_VARIANTS: dict[tuple[int,int], bytes] = {
    (0x84, 0xc1): bytes([0x31, 0xC0, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90]),  # dst=rax xor eax,eax
    (0x84, 0xc5): bytes([0x31, 0xC0, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90]),  # dst=rax xor eax,eax
    (0x8c, 0xca): bytes([0x31, 0xC9, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90]),  # dst=rcx xor ecx,ecx
    (0x8c, 0xcd): bytes([0x31, 0xC9, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90]),  # dst=rcx xor ecx,ecx
    (0x8c, 0xcf): bytes([0x31, 0xC9, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90]),  # dst=rcx xor ecx,ecx
    (0x8c, 0xd1): bytes([0x31, 0xC9, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90]),  # dst=rcx xor ecx,ecx
    (0x94, 0xd1): bytes([0x31, 0xD2, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90]),  # dst=rdx xor edx,edx
    (0x94, 0xd6): bytes([0x31, 0xD2, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90]),  # dst=rdx xor edx,edx
}
# Register sentinel for verify/apply commands (uses first Form-A occurrence)
_POOL_LOOKUP_FIND = bytes([0x48, 0x8b, 0x84, 0xc1, 0x17, 0x04, 0x00, 0x00])  # compat alias
_POOL_LOOKUP_REPL = bytes([0x31, 0xC0, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90])  # compat alias
PATCHES["P_READFILL_OBJPOOL_ALL_CLUSTERS"] = (
    0x2292e9d,  # sentinel: first Form-A file offset
    _POOL_LOOKUP_REPL,
)


# ── PostLoad bypass patches ────────────────────────────────────────────────────
# All AOT deserialization cluster PostLoad functions are patched to ret immediately.
# PostLoad normally installs type-test stubs and links canonical types, but since our
# ReadFill pool lookups now write 0 to all object fields, PostLoad crashes on null derefs.
# Bypassing PostLoad is safe for the boot stub path -- we just need the isolate to start.
_POSTLOADS_TO_BYPASS = [
    # CORRECTED addresses (old list was stale by -0x1000 and hit wrong functions). Only the
    # CLUSTER PostLoads (which null-deref on AOT-stripped fields). The ROOTS PostLoads
    # (VMDeser/ProgramDeser/UnitDeser) are intentionally NOT here — they populate base objects.
    (0x2294210, 'TypedDataViewDeserializationCluster::PostLoad'),
    (0x2294c60, 'RODataDeserializationCluster::PostLoad'),
    (0x2296b20, 'TypeArgumentsDeserializationCluster::PostLoad'),
    (0x22977f0, 'FunctionDeserializationCluster::PostLoad'),
    (0x2298710, 'FieldDeserializationCluster::PostLoad'),
    (0x2299430, 'KernelProgramInfoDeserializationCluster::PostLoad'),
    (0x2299ca0, 'CodeDeserializationCluster::PostLoad'),
    (0x229a9e0, 'ObjectPoolDeserializationCluster::PostLoad'),
    (0x229d1b0, 'TypeDeserializationCluster::PostLoad'),
    (0x229db50, 'FunctionTypeDeserializationCluster::PostLoad'),
    (0x229e500, 'RecordTypeDeserializationCluster::PostLoad'),
    (0x229eed0, 'TypeParameterDeserializationCluster::PostLoad'),
    (0x22a1c70, 'StringDeserializationCluster::PostLoad'),
    # ── ROOTS PostLoads REMOVED: these three addresses are STALE (off by 0x1000) and
    #    actually point at unrelated functions (0x22a20b0 = CompressedStackMaps::Handle),
    #    so ret-patching them corrupted VM-snapshot deserialization. Worse, the REAL roots
    #    PostLoads MUST run — VMDeserializationRoots::PostLoad (0x22a30b0) tail-calls
    #    set_vm_isolate_snapshot_object_table (populates the 1247 VM base objects), and
    #    UnitDeserializationRoots::PostLoad (0x22a39a0) calls LoadingUnit::set_base_objects.
    #    Bypassing them caused "Snapshot expects 1247 base objects, but deserializer provided 0".
    # (0x22a30b0, 'VMDeserializationRoots::PostLoad'),    — must RUN, do not bypass
    # (0x22a3480, 'ProgramDeserializationRoots::PostLoad') — re-add with correct addr only if it crashes
    # (0x22a39a0, 'UnitDeserializationRoots::PostLoad'),  — must RUN, do not bypass
]
for _va, _name in _POSTLOADS_TO_BYPASS:
    # The addresses above are VIRTUAL addresses (from objdump). apply() writes by FILE
    # offset, and the exec segment has vaddr = fileoffset + 0x1000, so the raw value must
    # be converted or the 0xC3 lands 0x1000 high — e.g. StringDeser::PostLoad (va 0x22a1c70)
    # was corrupting AddBaseObjects+0x350 (va 0x22a2c70), turning a `mov` into a `ret` that
    # popped a heap object off the stack and jumped into the Dart heap. Convert with va_to_file.
    PATCHES[f'P_POSTLOAD_BYPASS_{_va:x}'] = (va_to_file(_va), bytes([0xC3]))  # ret

# ── ImageReader::GetInstructionsAt anomalous-offset clamp ──────────────────────
# 8000+ isolate Code objects decode sane small signed instruction offsets, but a
# late one decodes 0x80380000 (sign-extended -0x7FC80000, ~2GB below the image
# base) -> unmapped ptr -> SIGSEGV in Code::InitializeCachedEntryPointsFrom.
# Clamp out-of-range offsets to 0 (returns base+1, a mapped address) so the
# snapshot finishes loading. Supersedes the kernel dl.rs Patch7 (which byte-match
# guards and auto-skips once 0x22cfe60 starts with 0xE9). Cave at file 0x1F44C60
# (free space after P_DEBUG_CAVE which ends at 0x1F44C5A).
_GI_HOOK_VA = 0x22cfe60
_GI_CAVE_FILE = 0x1F44C60
_GI_CAVE_VA = file_to_va(_GI_CAVE_FILE)
PATCHES["P_GETINSTR_CLAMP_HOOK"] = (va_to_file(_GI_HOOK_VA), jmp_rel32(_GI_HOOK_VA, _GI_CAVE_VA))
PATCHES["P_GETINSTR_CLAMP_CAVE"] = (_GI_CAVE_FILE, bytes([
    0x48, 0x8b, 0x47, 0x08,                    # mov rax, [rdi+8]      ; image base
    0x48, 0x63, 0xce,                          # movsxd rcx, esi       ; signed offset
    0x48, 0x81, 0xf9, 0x20, 0x50, 0x29, 0x00,  # cmp rcx, 0x295020     ; iso image size
    0x7f, 0x09,                                # jg  L_clamp
    0x48, 0x81, 0xf9, 0x00, 0x00, 0xf0, 0xff,  # cmp rcx, -0x100000
    0x7d, 0x02,                                # jge L_done
    0x31, 0xc9,                                # L_clamp: xor ecx, ecx
    0x48, 0x8d, 0x44, 0x08, 0x01,              # L_done:  lea rax, [rax+rcx+1]
    0xc3,                                      # ret
]))

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
        if name == "P_READFILL_OBJPOOL_ALL_CLUSTERS":
            # Mass-replace all pool lookup variants in the text segment
            # with xor dst_reg,dst_reg + nop padding (safe, no-op for deser stub purposes)
            count = 0
            DISP = bytes([0x17, 0x04, 0x00, 0x00])
            pos = 0x1953248
            while pos < 0x266AA30 - 8:
                if data[pos+4:pos+8] == DISP:
                    prefix = data[pos]
                    op = data[pos+1]
                    if 0x48 <= prefix <= 0x4F and op == 0x8B:
                        modrm = data[pos+2]
                        reg = (modrm >> 3) & 7
                        is_extended = (prefix & 4) != 0
                        if is_extended:
                            # xor rNd, rNd (r8d-r15d)
                            repl = bytes([0x45, 0x31, 0xC0 | (reg << 3) | reg])
                        else:
                            # xor rNd, rNd (eax-edi)
                            repl = bytes([0x31, 0xC0 | (reg << 3) | reg])
                        repl = repl + b"\x90" * (8 - len(repl))
                        data[pos : pos + 8] = repl
                        count += 1
                        pos += 8
                        continue
                pos += 1
            print(f"  [P_READFILL_OBJPOOL_ALL_CLUSTERS] replaced {count} pool lookups with xor-zero stubs")
        else:
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

    libapp_path = ROOT / "initramfs/system/flutter/libapp.so"
    extracted_hash = get_sdk_hash_from_elf(libapp_path) if libapp_path.exists() else None
    if extracted_hash:
        print(f"Extracted dynamic SDK hash from libapp.so: {extracted_hash.decode('ascii')}")
        target_hash = extracted_hash + b"\x00"
    else:
        print("Using fallback SDK hash 78da37fed6")
        target_hash = b"78da37fed6\x00"

    PATCHES["P_SDK_HASH"] = (0x188ced, target_hash)
    PATCHES["P_SDK_HASH_2"] = (0x3bd638, target_hash)
    PATCHES["P_SDK_HASH_3"] = (0xd81028, target_hash)

    data = bytearray(engine.read_bytes())

    all_patches = [
        "P1", "P2", "P3", "P4", "P5", "P6", "P10", "P9", "P9_CAVE", "P9_PROLOGUE", "P9_EPILOGUE",
        "P_RUNS_AOT",
        "P_JIT_AOT_CHECK", "P_ALLOW_ALL_DART_FLAGS",
        "P_FINISH_INIT_HOOK", "P_FINISH_INIT_CAVE", "P_FINISH_INIT_HOOK_2", "P_FINISH_INIT_CAVE_2",
        "P_INIT_TTS_HOOK", "P_INIT_TTS_CAVE",
        "P_BYPASS_TTS_INIT_IN_VM_CONSTANTS_PART1", "P_BYPASS_TTS_INIT_IN_VM_CONSTANTS_PART2", "P_VM_CONSTANTS_CAVE",
        "P_IS_PRECOMPILED_RUNTIME", "P_FLAG_PRECOMPILED_MODE_DEFAULT", "P_FLAG_PRECOMPILED_MODE_INITIAL",
        "P_BYPASS_VM_SERVICE_TASK",
        "P_BYPASS_SERVICE_ISOLATE_RUN",
        "P_DEBUG_CAVE",
        # AOT strips class-name strings, so finalizing them calls Utf8::Length on a null String
        # → crash. This bypass (in a non-reader code region) is legitimately needed for AOT.
        "P_BYPASS_FINALIZE_CLASS_NAMES",
        # ENV-NEEDED (not a deser hack): redirects Dart's ReadInstructions to the bare-metal
        # cave (P_DEBUG_CAVE) that handles OSCortex's SEPARATE instructions image. Without it
        # the engine's inline-instructions reader desyncs the stream on code/ObjectPool clusters
        # → garbage Array length → OOM.
        "P_READ_INSTRUCTIONS",
        # Clamp the one anomalous isolate Code instruction offset (0x80380000) that
        # would otherwise resolve to an unmapped ptr and crash entry-point setup.
        "P_GETINSTR_CLAMP_HOOK", "P_GETINSTR_CLAMP_CAVE",
        # ── INTENTIONALLY OMITTED: P_SDK_HASH*, P_SNAPSHOT_FEATURES_CHECK,
        #    P_BYPASS_BASE_OBJECTS_CHECK, P_BYPASS_BASE_OBJECTS_COMPARE, and all deser-content
        #    hacks. The reader-region bypasses (FEATURES_CHECK + BASE_OBJECTS_*) skip stream reads
        #    and desync the deserialization cursor → garbage Array length → OOM. Removing them
        #    makes deserialization correct.
    ]
    # Bypass CLUSTER PostLoads at their CORRECTED addresses — they null-deref on AOT-stripped
    # fields. (Roots PostLoads are excluded from the list so they run and populate base objects.)
    all_patches.extend([f"P_POSTLOAD_BYPASS_{fo:x}" for fo, _ in _POSTLOADS_TO_BYPASS])


    if args.verify:
        names = all_patches if args.apply_all else None
        if names:
            errors = []
            for n in names:
                errors.extend(verify(data, n))
        else:
            errors = verify(data, "P1")
            for n in all_patches[1:]:
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
        all_patches
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
