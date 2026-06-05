import capstone
from pathlib import Path

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
engine_path = Path("tools/flutter-engine/libflutter_engine.so")

bak_data = bak_path.read_bytes()
eng_data = engine_path.read_bytes()

offsets = [
    (0x2298f74, "P_READFILL_OBJPOOL_0"),
    (0x2299024, "P_READFILL_OBJPOOL"),
    (0x22992a4, "P_READFILL_STACKMAPS"),
]

md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)

for off, name in offsets:
    print(f"\n--- {name} at file offset {hex(off)} ---")
    
    # Pristine
    print("Pristine:")
    chunk = bak_data[off : off + 20]
    for i in md.disasm(chunk, off):
        print(f"  0x{i.address:x}: {i.mnemonic} {i.op_str}")
        
    # Patched
    print("Patched:")
    chunk = eng_data[off : off + 20]
    for i in md.disasm(chunk, off):
        print(f"  0x{i.address:x}: {i.mnemonic} {i.op_str}")
