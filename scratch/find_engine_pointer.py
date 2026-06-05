import struct

with open("tools/flutter-engine/libflutter_engine.so", "rb") as f:
    data = f.read()

pattern = struct.pack("<I", 0x26343003)

print("Searching libflutter_engine.so for 0x26343003...")
for i in range(0, len(data) - 3, 4):
    val = struct.unpack("<I", data[i:i+4])[0]
    if val == 0x26343003:
        print(f"Found 0x26343003 at file offset: 0x{i:x}")

print("Searching libflutter_engine.so for any value in [0x26343000, 0x26343010]...")
for i in range(0, len(data) - 3, 4):
    val = struct.unpack("<I", data[i:i+4])[0]
    if 0x26343000 <= val <= 0x26343010:
        print(f"Found 0x{val:x} at file offset: 0x{i:x}")
