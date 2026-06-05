import struct

with open("initramfs/system/flutter/libapp.so", "rb") as f:
    data = f.read()

print(f"File size: {len(data)}")

# Search for any 8-byte value (unaligned) in [0x1_0000_0000, 0x1_0100_0000)
matches = []
for i in range(len(data) - 7):
    val = struct.unpack_from("<Q", data, i)[0]
    if 0x1_0000_0000 <= val < 0x1_0100_0000:
        matches.append((i, val))

print(f"Found {len(matches)} pointers:")
for offset, val in matches:
    print(f"  Offset: {offset:#x} (byte offset), Value: {val:#x}")
