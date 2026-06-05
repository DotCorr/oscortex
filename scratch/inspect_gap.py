import sys
from pathlib import Path

libapp_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/flutter/libapp.so")
data = libapp_path.read_bytes()

gap_start = 0x352232
gap_end = 0x354000

gap_data = data[gap_start:gap_end]
non_zero = [(i + gap_start, b) for i, b in enumerate(gap_data) if b != 0]

print(f"Gap size: {len(gap_data)} bytes")
print(f"Non-zero bytes count in gap: {len(non_zero)}")
if non_zero:
    print("First 20 non-zero bytes:")
    for addr, b in non_zero[:20]:
        print(f"  {hex(addr)}: {hex(b)}")
else:
    print("Entire gap is zero/padding in file.")
