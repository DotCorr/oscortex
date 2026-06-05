import struct

with open("initramfs/system/flutter/libapp.so", "rb") as f:
    data = f.read()

vm_data = data[0x340 : 0x340 + 0x9830]
iso_data = data[0x9b80 : 0x9b80 + 0x1c1ff0]

pattern = struct.pack("<I", 0x26343003)

print("Searching for 0x26343003...")
for i in range(0, len(vm_data) - 3, 4):
    val = struct.unpack("<I", vm_data[i:i+4])[0]
    if val == 0x26343003:
        print(f"Found in vm_data at relative offset: 0x{i:x}")

for i in range(0, len(iso_data) - 3, 4):
    val = struct.unpack("<I", iso_data[i:i+4])[0]
    if val == 0x26343003:
        print(f"Found in iso_data at relative offset: 0x{i:x} (file offset: 0x{0x9b80+i:x})")

# Also search for similar values just in case
print("Searching for any value in [0x26343000, 0x26343010]...")
for i in range(0, len(iso_data) - 3, 4):
    val = struct.unpack("<I", iso_data[i:i+4])[0]
    if 0x26343000 <= val <= 0x26343010:
        print(f"Found val 0x{val:x} in iso_data at relative offset: 0x{i:x}")
