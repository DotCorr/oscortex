#!/usr/bin/env python3
import os
import re
import struct

LOAD_VADDR = 0x400000
PHOFF = 64
PHENT = 56
CODE_FILE_OFFSET = PHOFF + PHENT

W = 200
H = 120
EVBUF_ALLOC = 256

ABI_CONST_SPEC = {
    "EMBEDDER_ABI_VERSION": {"type": "u64", "format": "dec"},
    "SYS_SURFACE_CREATE": {"type": "u64", "format": "hex"},
    "SYS_SURFACE_MOVE": {"type": "u64", "format": "hex"},
    "SYS_SURFACE_DESTROY": {"type": "u64", "format": "hex"},
    "SYS_SURFACE_UPLOAD_RGBA32": {"type": "u64", "format": "hex"},
    "SYS_SURFACE_PRESENT": {"type": "u64", "format": "hex"},
    "SYS_FB_SIZE_PACKED": {"type": "u64", "format": "hex"},
    "SYS_VSYNC_COUNTER": {"type": "u64", "format": "hex"},
    "SYS_VSYNC_WAIT_NONBLOCK": {"type": "u64", "format": "hex"},
    "SYS_SURFACE_OWNER": {"type": "u64", "format": "hex"},
    "SYS_SURFACE_Z_GET": {"type": "u64", "format": "hex"},
    "SYS_SURFACE_Z_SET": {"type": "u64", "format": "hex"},
    "SYS_SURFACE_GEOMETRY_GET": {"type": "u64", "format": "hex"},
    "SYS_SURFACE_GEOMETRY_SET": {"type": "u64", "format": "hex"},
    "SYS_SURFACE_VISIBILITY_GET": {"type": "u64", "format": "hex"},
    "SYS_SURFACE_VISIBILITY_SET": {"type": "u64", "format": "hex"},
    "SYS_SURFACE_CLIP_SET": {"type": "u64", "format": "hex"},
    "SYS_SURFACE_DAMAGE_SET": {"type": "u64", "format": "hex"},
    "SYS_SURFACE_DAMAGE_GET": {"type": "u64", "format": "hex"},
    "SYS_SURFACE_FLIP": {"type": "u64", "format": "hex"},
    "SYS_WM_EVENT_POLL": {"type": "u64", "format": "hex"},
    "SYS_WM_EVENT_READ": {"type": "u64", "format": "hex"},
    "SYS_WM_EVENT_INJECT": {"type": "u64", "format": "hex"},
    "SYS_WM_EVENT_WAIT": {"type": "u64", "format": "hex"},
    "SYS_EMBEDDER_ABI_VERSION": {"type": "u64", "format": "hex"},
    "SYS_WM_EVENT_SIZE": {"type": "u64", "format": "hex"},
    "SYS_WM_EVENT_STATS": {"type": "u64", "format": "hex"},
    "SYS_WM_FOCUS_PID_GET": {"type": "u64", "format": "hex"},
    "SYS_WM_FOCUS_SURFACE_SET": {"type": "u64", "format": "hex"},
    "SYS_WM_FOCUS_MIRROR_GET": {"type": "u64", "format": "hex"},
    "SYS_WM_FOCUS_MIRROR_SET": {"type": "u64", "format": "hex"},
    "SYS_APP_NOTIFY": {"type": "u64", "format": "hex"},
    "SYS_PROC_SURFACE_COUNT": {"type": "u64", "format": "hex"},
    "EV_VSYNC": {"type": "u32", "format": "dec"},
    "EV_POINTER": {"type": "u32", "format": "dec"},
    "EV_KEY": {"type": "u32", "format": "dec"},
    "EV_APP": {"type": "u32", "format": "dec"},
    "EV_FOCUS": {"type": "u32", "format": "dec"},
    "APP_LAUNCH": {"type": "u32", "format": "dec"},
    "APP_TERMINATE": {"type": "u32", "format": "dec"},
    "APP_PAUSE": {"type": "u32", "format": "dec"},
    "APP_RESUME": {"type": "u32", "format": "dec"},
}

EXPECTED_WM_EVENT_FIELDS = [
    ("seq", "u64"),
    ("kind", "u32"),
    ("flags", "u32"),
    ("a", "u64"),
    ("b", "u64"),
]

def _assert_abi_contract_shape(text, abi_rs_path):
    if "#[repr(C)]" not in text:
        raise RuntimeError(f"embedder ABI drift in {abi_rs_path}: missing #[repr(C)] on WmEvent")

    struct_match = re.search(r"pub struct WmEvent\s*\{([^}]*)\}", text, re.S)
    if not struct_match:
        raise RuntimeError(f"embedder ABI drift in {abi_rs_path}: missing pub struct WmEvent")

    fields = re.findall(r"pub\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*:\s*([a-z0-9_]+)\s*,", struct_match.group(1))
    if fields != EXPECTED_WM_EVENT_FIELDS:
        raise RuntimeError(
            f"embedder ABI drift in {abi_rs_path}: WmEvent fields/type/order changed; "
            f"expected {EXPECTED_WM_EVENT_FIELDS}, got {fields}"
        )

    wm_size_line = re.search(r"pub const WM_EVENT_SIZE: usize = ([^;]+);", text)
    if not wm_size_line:
        raise RuntimeError(f"embedder ABI drift in {abi_rs_path}: missing WM_EVENT_SIZE declaration")
    if wm_size_line.group(1).strip() != "core::mem::size_of::<WmEvent>()":
        raise RuntimeError(
            f"embedder ABI drift in {abi_rs_path}: WM_EVENT_SIZE expression changed from canonical size_of::<WmEvent>()"
        )

def _parse_numeric_const(raw_expr, name, expected_fmt, abi_rs_path):
    expr = raw_expr.strip()
    if expected_fmt == "hex":
        if not re.fullmatch(r"0x[0-9A-Fa-f]+", expr):
            raise RuntimeError(
                f"embedder ABI drift in {abi_rs_path}: {name} must be hex literal (0x...), got {expr!r}"
            )
    elif expected_fmt == "dec":
        if not re.fullmatch(r"[0-9]+", expr):
            raise RuntimeError(
                f"embedder ABI drift in {abi_rs_path}: {name} must be decimal literal, got {expr!r}"
            )
    else:
        raise RuntimeError(f"internal error: unsupported expected format {expected_fmt!r} for {name}")
    return int(expr, 0)

def _validate_contiguous_syscall_ranges(consts, abi_rs_path):
    compositor = [
        consts["SYS_SURFACE_CREATE"],
        consts["SYS_SURFACE_MOVE"],
        consts["SYS_SURFACE_DESTROY"],
        consts["SYS_SURFACE_UPLOAD_RGBA32"],
        consts["SYS_SURFACE_PRESENT"],
        consts["SYS_FB_SIZE_PACKED"],
        consts["SYS_VSYNC_COUNTER"],
        consts["SYS_VSYNC_WAIT_NONBLOCK"],
        consts["SYS_SURFACE_OWNER"],
        consts["SYS_SURFACE_Z_GET"],
        consts["SYS_SURFACE_Z_SET"],
        consts["SYS_SURFACE_GEOMETRY_GET"],
        consts["SYS_SURFACE_GEOMETRY_SET"],
        consts["SYS_SURFACE_VISIBILITY_GET"],
        consts["SYS_SURFACE_VISIBILITY_SET"],
        consts["SYS_SURFACE_CLIP_SET"],
        consts["SYS_SURFACE_DAMAGE_SET"],
        consts["SYS_SURFACE_DAMAGE_GET"],
        consts["SYS_SURFACE_FLIP"],
    ]
    wm = [
        consts["SYS_WM_EVENT_POLL"],
        consts["SYS_WM_EVENT_READ"],
        consts["SYS_WM_EVENT_INJECT"],
        consts["SYS_WM_EVENT_WAIT"],
    ]
    introspection = [
        consts["SYS_EMBEDDER_ABI_VERSION"],
        consts["SYS_WM_EVENT_SIZE"],
        consts["SYS_WM_EVENT_STATS"],
        consts["SYS_WM_FOCUS_PID_GET"],
        consts["SYS_WM_FOCUS_SURFACE_SET"],
        consts["SYS_WM_FOCUS_MIRROR_GET"],
        consts["SYS_WM_FOCUS_MIRROR_SET"],
    ]
    app_proc = [
        consts["SYS_APP_NOTIFY"],
        consts["SYS_PROC_SURFACE_COUNT"],
    ]

    def is_contiguous(values):
        return values == list(range(values[0], values[0] + len(values)))

    if not is_contiguous(compositor):
        raise RuntimeError(
            f"embedder ABI drift in {abi_rs_path}: compositor syscall block must remain contiguous; got {compositor}"
        )
    if not is_contiguous(wm):
        raise RuntimeError(
            f"embedder ABI drift in {abi_rs_path}: WM syscall block must remain contiguous; got {wm}"
        )
    if not is_contiguous(introspection):
        raise RuntimeError(
            f"embedder ABI drift in {abi_rs_path}: introspection syscall block must remain contiguous; got {introspection}"
        )
    if not is_contiguous(app_proc):
        raise RuntimeError(
            f"embedder ABI drift in {abi_rs_path}: app/process syscall block must remain contiguous; got {app_proc}"
        )

def load_embedder_abi_constants(abi_rs_path):
    with open(abi_rs_path, "r", encoding="utf-8") as f:
        text = f.read()

    _assert_abi_contract_shape(text, abi_rs_path)

    parsed = {}
    for name, type_name, expr in re.findall(r"pub const ([A-Z0-9_]+):\s*([a-z0-9_]+)\s*=\s*([^;]+);", text):
        parsed[name] = {"type": type_name.strip(), "expr": expr.strip()}

    consts = {}
    missing = [k for k in ABI_CONST_SPEC if k not in parsed]
    if missing:
        raise RuntimeError(f"embedder ABI drift in {abi_rs_path}: missing constants: {', '.join(missing)}")

    for name, spec in ABI_CONST_SPEC.items():
        got_type = parsed[name]["type"]
        if got_type != spec["type"]:
            raise RuntimeError(
                f"embedder ABI drift in {abi_rs_path}: {name} type changed, expected {spec['type']}, got {got_type}"
            )

        value = _parse_numeric_const(parsed[name]["expr"], name, spec["format"], abi_rs_path)
        if spec["type"] == "u32" and not (0 <= value <= 0xFFFFFFFF):
            raise RuntimeError(f"embedder ABI drift in {abi_rs_path}: {name}={value} out of u32 range")
        if spec["type"] == "u64" and not (0 <= value <= 0xFFFFFFFFFFFFFFFF):
            raise RuntimeError(f"embedder ABI drift in {abi_rs_path}: {name}={value} out of u64 range")
        consts[name] = value

    _validate_contiguous_syscall_ranges(consts, abi_rs_path)
    return consts

ABI_RS_PATH = os.path.join("kernel", "src", "embedder", "abi.rs")
ABI = load_embedder_abi_constants(ABI_RS_PATH)

EMBEDDER_ABI_VERSION = ABI["EMBEDDER_ABI_VERSION"]
SYS_SURFACE_CREATE = ABI["SYS_SURFACE_CREATE"]
SYS_SURFACE_MOVE = ABI["SYS_SURFACE_MOVE"]
SYS_SURFACE_UPLOAD_RGBA32 = ABI["SYS_SURFACE_UPLOAD_RGBA32"]
SYS_SURFACE_PRESENT = ABI["SYS_SURFACE_PRESENT"]
SYS_WM_EVENT_INJECT = ABI["SYS_WM_EVENT_INJECT"]
SYS_WM_EVENT_WAIT = ABI["SYS_WM_EVENT_WAIT"]
SYS_EMBEDDER_ABI_VERSION = ABI["SYS_EMBEDDER_ABI_VERSION"]
SYS_WM_EVENT_SIZE = ABI["SYS_WM_EVENT_SIZE"]
SYS_WM_EVENT_STATS = ABI["SYS_WM_EVENT_STATS"]
EV_APP = ABI["EV_APP"]

def emit_mov_rax_imm32(b, imm):
    b += b"\x48\xC7\xC0" + struct.pack("<I", imm & 0xFFFFFFFF)

def emit_mov_rdi_imm32(b, imm):
    b += b"\x48\xC7\xC7" + struct.pack("<I", imm & 0xFFFFFFFF)

def emit_mov_rsi_imm64(b, imm):
    b += b"\x48\xBE" + struct.pack("<Q", imm & 0xFFFFFFFFFFFFFFFF)

def emit_mov_rdx_imm32(b, imm):
    b += b"\x48\xC7\xC2" + struct.pack("<I", imm & 0xFFFFFFFF)

def emit_mov_rcx_imm32(b, imm):
    b += b"\x48\xC7\xC1" + struct.pack("<I", imm & 0xFFFFFFFF)

def emit_cmp_rax_rcx(b):
    b += b"\x48\x39\xC8"

def emit_jne_rel32_placeholder(b):
    rel_off = len(b) + 2
    b += b"\x0F\x85\x00\x00\x00\x00"
    return rel_off

def emit_mov_rsi_r13(b):
    b += b"\x4C\x89\xEE"

code = bytearray()

# abi = syscall(SYS_EMBEDDER_ABI_VERSION)
emit_mov_rax_imm32(code, SYS_EMBEDDER_ABI_VERSION)
code += b"\x0F\x05"

# if abi != EMBEDDER_ABI_VERSION: halt loop
emit_mov_rcx_imm32(code, EMBEDDER_ABI_VERSION)
emit_cmp_rax_rcx(code)
abi_mismatch_jne_rel32_off = emit_jne_rel32_placeholder(code)

# ev_size = syscall(SYS_WM_EVENT_SIZE)
emit_mov_rax_imm32(code, SYS_WM_EVENT_SIZE)
code += b"\x0F\x05"

# save event size in r13 for later wm_event_wait(len=ev_size)
code += b"\x49\x89\xC5"  # mov r13, rax

# id = syscall(SYS_SURFACE_CREATE, width, height)
emit_mov_rax_imm32(code, SYS_SURFACE_CREATE)
emit_mov_rdi_imm32(code, W)
emit_mov_rsi_imm64(code, H)
code += b"\x0F\x05"

# save id in r12
code += b"\x49\x89\xC4"  # mov r12, rax

# syscall(SYS_SURFACE_MOVE, id, packed_xy=(x<<32)|y, z=30)
packed_xy = (40 << 32) | 120
emit_mov_rax_imm32(code, SYS_SURFACE_MOVE)
code += b"\x4C\x89\xE7"  # mov rdi, r12
emit_mov_rsi_imm64(code, packed_xy)
emit_mov_rdx_imm32(code, 30)
code += b"\x0F\x05"

# syscall(SYS_SURFACE_UPLOAD_RGBA32, id, payload_ptr, payload_len)
emit_mov_rax_imm32(code, SYS_SURFACE_UPLOAD_RGBA32)
code += b"\x4C\x89\xE7"  # mov rdi, r12
payload_ptr_off = len(code)
emit_mov_rsi_imm64(code, 0)  # patched later
payload_len = W * H * 4
emit_mov_rdx_imm32(code, payload_len)
code += b"\x0F\x05"

# syscall(SYS_SURFACE_PRESENT, id)
emit_mov_rax_imm32(code, SYS_SURFACE_PRESENT)
code += b"\x4C\x89\xE7"  # mov rdi, r12
code += b"\x0F\x05"

# syscall(SYS_WM_EVENT_INJECT, EV_APP, 0x1234, 0x5678)
emit_mov_rax_imm32(code, SYS_WM_EVENT_INJECT)
emit_mov_rdi_imm32(code, EV_APP)
emit_mov_rsi_imm64(code, 0x1234)
emit_mov_rdx_imm32(code, 0x5678)
code += b"\x0F\x05"

# syscall(SYS_WM_EVENT_WAIT, evbuf_ptr, ev_size, 8)
emit_mov_rax_imm32(code, SYS_WM_EVENT_WAIT)
evbuf_ptr_off = len(code)
emit_mov_rdi_imm32(code, 0)  # patched later
emit_mov_rsi_r13(code)
emit_mov_rdx_imm32(code, 8)
code += b"\x0F\x05"

# wait for another event (exercise streaming event loop contract)
emit_mov_rax_imm32(code, SYS_WM_EVENT_WAIT)
evbuf2_ptr_off = len(code)
emit_mov_rdi_imm32(code, 0)  # patched later
emit_mov_rsi_r13(code)
emit_mov_rdx_imm32(code, 16)
code += b"\x0F\x05"

# syscall(SYS_WM_EVENT_STATS) to sample queue health telemetry
emit_mov_rax_imm32(code, SYS_WM_EVENT_STATS)
code += b"\x0F\x05"

# write marker text through syscall write(1, "[init] compositor frame\n", len)
msg = b"[init] compositor frame\n"
emit_mov_rax_imm32(code, 1)
emit_mov_rdi_imm32(code, 1)
msg_ptr_off = len(code)
emit_mov_rsi_imm64(code, 0)  # patched later
emit_mov_rdx_imm32(code, len(msg))
code += b"\x0F\x05"

# hlt loop
hlt_loop_off = len(code)
code += b"\xF4\xEB\xFD"

msg_file_off = CODE_FILE_OFFSET + len(code)
payload_file_off = msg_file_off + len(msg)
evbuf_file_off = payload_file_off + payload_len
evbuf2_file_off = evbuf_file_off + EVBUF_ALLOC

msg_vaddr = LOAD_VADDR + msg_file_off
payload_vaddr = LOAD_VADDR + payload_file_off
evbuf_vaddr = LOAD_VADDR + evbuf_file_off
evbuf2_vaddr = LOAD_VADDR + evbuf2_file_off

# Patch immediates.
struct.pack_into("<Q", code, payload_ptr_off + 2, payload_vaddr)
struct.pack_into("<Q", code, msg_ptr_off + 2, msg_vaddr)
struct.pack_into("<I", code, evbuf_ptr_off + 3, evbuf_vaddr & 0xFFFFFFFF)
struct.pack_into("<I", code, evbuf2_ptr_off + 3, evbuf2_vaddr & 0xFFFFFFFF)

# patch ABI mismatch jump target to halt loop
abi_jne_next = abi_mismatch_jne_rel32_off + 4
abi_jne_rel = hlt_loop_off - abi_jne_next
struct.pack_into("<i", code, abi_mismatch_jne_rel32_off, abi_jne_rel)

# RGBA payload (horizontal gradient + vertical mod).
payload = bytearray(payload_len)
for y in range(H):
    for x in range(W):
        i = (y * W + x) * 4
        r = (x * 255) // max(1, W - 1)
        g = (y * 255) // max(1, H - 1)
        b = 200
        a = 255
        payload[i + 0] = r
        payload[i + 1] = g
        payload[i + 2] = b
        payload[i + 3] = a

segment = bytes(code) + msg + bytes(payload) + bytes(EVBUF_ALLOC * 2)
total = CODE_FILE_OFFSET + len(segment)
entry = LOAD_VADDR + CODE_FILE_OFFSET

elf = bytearray(64)
elf[0:4] = b"\x7fELF"
elf[4] = 2  # ELFCLASS64
elf[5] = 1  # little-endian
elf[6] = 1  # current
struct.pack_into("<H", elf, 16, 2)   # ET_EXEC
struct.pack_into("<H", elf, 18, 62)  # EM_X86_64
struct.pack_into("<I", elf, 20, 1)
struct.pack_into("<Q", elf, 24, entry)
struct.pack_into("<Q", elf, 32, PHOFF)
struct.pack_into("<H", elf, 52, 64)
struct.pack_into("<H", elf, 54, PHENT)
struct.pack_into("<H", elf, 56, 1)

phdr = bytearray(PHENT)
struct.pack_into("<I", phdr, 0, 1)           # PT_LOAD
struct.pack_into("<I", phdr, 4, 5)           # PF_R|PF_X
struct.pack_into("<Q", phdr, 8, 0)           # p_offset
struct.pack_into("<Q", phdr, 16, LOAD_VADDR) # p_vaddr
struct.pack_into("<Q", phdr, 24, LOAD_VADDR) # p_paddr
struct.pack_into("<Q", phdr, 32, total)      # p_filesz
struct.pack_into("<Q", phdr, 40, total)      # p_memsz
struct.pack_into("<Q", phdr, 48, 0x1000)     # p_align

out = bytes(elf) + bytes(phdr) + segment
os.makedirs("initramfs", exist_ok=True)
with open("initramfs/init", "wb") as f:
    f.write(out)
os.chmod("initramfs/init", 0o755)
print(f"wrote initramfs/init ({len(out)} bytes), entry=0x{entry:x}, payload={payload_len} bytes")

