import sys

path = "/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/libflutter_engine.so"
offset = 0x2346f60

try:
    with open(path, "rb") as f:
        f.seek(offset - 32)
        data = f.read(64)
        print("Bytes around offset 0x{:x}:".format(offset))
        for i in range(0, 64, 16):
            chunk = data[i:i+16]
            hex_str = " ".join("{:02x}".format(b) for b in chunk)
            chunk_offset = offset - 32 + i
            print("0x{:x}: {}".format(chunk_offset, hex_str))
except Exception as e:
    print("Error:", e)
