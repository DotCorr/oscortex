import struct

libapp_path = "initramfs/system/flutter/libapp.so"
with open(libapp_path, "rb") as f:
    data = f.read()

print(f"libapp.so size: {len(data)} bytes")

# Search for 64-bit pointer patterns
# We look for 0x15c686006 (or similar in the cage range)
target_64 = 0x15c686006
target_32_comp = 0x2e343003 # 0x5c686006 >> 1

print("Searching for 64-bit pointer...")
for i in range(0, len(data) - 8, 4):
    val = struct.unpack_from("<Q", data, i)[0]
    if val == target_64 or (val & 0xfffffffffffff000) == 0x15c686000:
        print(f"Found 64-bit pointer at offset {hex(i)}: {hex(val)}")

print("Searching for 32-bit compressed pointer...")
for i in range(0, len(data) - 4, 4):
    val = struct.unpack_from("<I", data, i)[0]
    if val == target_32_comp or val == 0x2e343003:
        print(f"Found 32-bit compressed pointer at offset {hex(i)}: {hex(val)}")
