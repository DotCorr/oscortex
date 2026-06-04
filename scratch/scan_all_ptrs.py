import struct
from pathlib import Path

def main():
    path = Path("./initramfs/system/flutter/libapp.so")
    if not path.exists():
        print("libapp.so not found")
        return
    data = path.read_bytes()
    
    code_ptrs = []
    data_ptrs = []
    
    for i in range(0, len(data) - 4, 4):
        val = struct.unpack("<I", data[i:i+4])[0]
        if 0x2000_0000 <= val < 0x2800_0000:
            c = 0x1_0000_0000 + (val << 1)
            c_addr = c & ~1
            if (c & 0x7) in (6, 7):
                code_ptrs.append(c_addr)
            else:
                data_ptrs.append(c_addr)
                
    print(f"Total compressed code pointers: {len(code_ptrs)}")
    print(f"Total compressed data pointers: {len(data_ptrs)}")
    
    # Let's cluster code pointers by grouping them into ranges that are within 5MB of each other
    code_ptrs.sort()
    clusters = []
    if code_ptrs:
        curr_min = code_ptrs[0]
        curr_max = code_ptrs[0]
        for p in code_ptrs[1:]:
            if p - curr_max < 0x500_000: # 5MB gap max
                curr_max = p
            else:
                clusters.append((curr_min, curr_max, len([x for x in code_ptrs if curr_min <= x <= curr_max])))
                curr_min = p
                curr_max = p
        clusters.append((curr_min, curr_max, len([x for x in code_ptrs if curr_min <= x <= curr_max])))
        
    print("Code pointer clusters:")
    for start, end, count in clusters:
        print(f"  Range [{hex(start)}, {hex(end)}]: count={count}")

if __name__ == '__main__':
    main()

