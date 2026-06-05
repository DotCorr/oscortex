import struct
from pathlib import Path

def main():
    path = Path("initramfs/system/flutter/libapp.so")
    data = path.read_bytes()
    
    decompressed_ptrs = []
    for i in range(0, len(data) - 4, 4):
        val = struct.unpack("<I", data[i:i+4])[0]
        if 0x2000_0000 <= val < 0x2800_0000:
            c = 0x1_0000_0000 + (val << 1)
            if (c & 0x7) in (6, 7):
                decompressed_ptrs.append((i, val, c & ~1))
                
    print(f"Total pointers: {len(decompressed_ptrs)}")
    print("First 20 pointers:")
    for offset, val, c_addr in decompressed_ptrs[:20]:
        print(f"  File Offset {hex(offset)}: raw_val={hex(val)} decompressed={hex(c_addr)}")

if __name__ == '__main__':
    main()
