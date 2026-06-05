import sys
from pathlib import Path

libapp_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/flutter/libapp.so")
if not libapp_path.exists():
    print("libapp.so not found")
    sys.exit(1)

data = libapp_path.read_bytes()
offset = 0x1192e2

print(f"Bytes at offset {hex(offset)} (len={len(data)}):")
chunk = data[offset - 32 : offset + 32]
for i in range(0, len(chunk), 16):
    hex_str = " ".join("{:02x}".format(b) for b in chunk[i:i+16])
    print(f"{hex(offset - 32 + i)}: {hex_str}")
