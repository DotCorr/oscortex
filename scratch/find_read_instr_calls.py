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
data = bak_path.read_bytes()

# Search for call instruction calling 0x2291070.
# Call relative 32-bit: E8 <disp32>
# Since the text segment is large, let's scan it.
calls = []
pos = 0x1953248
limit = 0x266AA30
while pos < limit - 5:
    if data[pos] == 0xe8:
        disp = int.from_bytes(data[pos+1:pos+5], byteorder='little', signed=True)
        target = (pos - 0x1953248 + 0x1954248) + 5 + disp
        if target == 0x2290ef0:
            calls.append(pos - 0x1953248 + 0x1954248)
    pos += 1

print(f"Found calls to ReadInstructions at VAs: {[hex(c) for c in calls]}")

# Disassemble around the found calls
md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
for call_va in calls:
    print(f"\n--- Disassembly around {hex(call_va)} ---")
    file_off = va_to_file(call_va - 0x20)
    chunk = data[file_off : file_off + 0x40]
    for i in md.disasm(chunk, call_va - 0x20):
        print(f"0x{i.address:x}: {i.mnemonic} {i.op_str}")
