import struct
from pathlib import Path

def main():
    path = Path("./initramfs/system/flutter/libapp.so")
    if not path.exists():
        print("libapp.so not found")
        return
    data = path.read_bytes()
    print(f"libapp.so size: {len(data)}")
    
    # Search for any 8-byte aligned 64-bit word
    for i in range(0, len(data) - 8, 8):
        val = struct.unpack("<Q", data[i:i+8])[0]
        # Check if it starts with 0x14f41
        if (val >> 16) == 0x14f41:
            print(f"Aligned 64-bit pointer found at {hex(i)}: {hex(val)}")
        elif (val >> 16) == 0x10000 or (val >> 20) == 0x1400:
            # Also check for other potential AOT snapshot pointers
            if 0x14f410000 <= val <= 0x14f420000:
                print(f"Aligned 64-bit pointer found at {hex(i)}: {hex(val)}")

    # Search for 4-byte aligned 32-bit values
    for i in range(0, len(data) - 4, 4):
        val = struct.unpack("<I", data[i:i+4])[0]
        # Decompress val using cage base shift to see if it targets 0x1_4f41_xxxx
        # c = 0x1_0000_0000 + (val << 1)
        c = 0x1_0000_0000 + (val << 1)
        if 0x14f410000 <= c <= 0x14f420000:
            print(f"Aligned 32-bit compressed pointer found at {hex(i)}: val32={hex(val)} -> decompressed={hex(c)}")

if __name__ == '__main__':
    main()
