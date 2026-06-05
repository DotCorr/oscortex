import capstone
from pathlib import Path

# Load segments mapping from engine_patch.py to translate VA to file offset
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

bak_path = Path("tools/flutter-engine/libflutter_engine.so.bak")
data = bak_path.read_bytes()

# Disassemble ArrayDeserializationCluster::ReadFill
start_va = 0x22a1400
end_va = 0x22a1500
file_start = va_to_file(start_va)
file_end = va_to_file(end_va)

chunk = data[file_start:file_end]

md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
for i in md.disasm(chunk, start_va):
    print(f"0x{i.address:x}: {i.mnemonic} {i.op_str}")
