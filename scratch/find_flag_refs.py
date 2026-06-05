import struct

lib = "tools/flutter-engine/libflutter_engine.so"
data = open(lib, "rb").read()

# Load segments VA to file offset
LOAD_SEGMENTS = (
    (0x0000000, 0x0000000),
    (0x1954248, 0x1953248),
    (0x266CA30, 0x266AA30),
    (0x26F3E00, 0x26F0E00),
)

def file_to_va(fo: int) -> int:
    for vbase, fbase in reversed(LOAD_SEGMENTS):
        if fo >= fbase:
            return fo - fbase + vbase
    return fo

def va_to_file(va: int) -> int:
    for vbase, fbase in reversed(LOAD_SEGMENTS):
        if va >= vbase:
            return va - vbase + fbase
    return va

target_va = 0x271f4a1

# Scan for RIP-relative instructions referencing target_va
# Instruction format: e.g. 8a 05 [4-byte offset] (mov al, [rip + offset])
# or 38 05 [4-byte offset] (cmp [rip + offset], al)
for i in range(0, 0x266CA30):
    if i >= len(data) - 6: break
    # check for common modrm patterns with RIP-relative addressing (disp32)
    # modrm byte: 05, 0d, 15, 1d, 25, 2d, 35, 3d, 85, 8d, 95, 9d, a5, ad, b5, bd, c5, cd, d5, dd, e5, ed, f5, fd etc (lower 3 bits are 101 => 5)
    modrm = data[i+1]
    if (modrm & 7) == 5:
        # potential rip-relative instruction
        disp = struct.unpack("<i", data[i+2:i+6])[0]
        va = file_to_va(i)
        dest_va = (va + 6 + disp) & 0xffffffff
        if dest_va == target_va:
            # Let's print the instruction bytes
            op = data[i:i+6]
            print(f"Ref at VA 0x{va:x} (file offset 0x{i:x}): op={op.hex()} disp={disp:x}")
