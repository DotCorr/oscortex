import struct
from collections import Counter
from pathlib import Path

def main():
    path = Path("./initramfs/system/flutter/libapp.so")
    if not path.exists():
        print("libapp.so not found")
        return
    data = path.read_bytes()
    
    vm_instr_off = 0x1cc000
    iso_instr_off = 0x1d2e80
    
    vm_instr_size = 0x6e70
    iso_instr_size = 0x253b70
    
    decompressed_ptrs = []
    for i in range(0, len(data) - 4, 4):
        val = struct.unpack("<I", data[i:i+4])[0]
        if 0x2000_0000 <= val < 0x2800_0000:
            c = 0x1_0000_0000 + (val << 1)
            if (c & 0x7) in (6, 7):
                decompressed_ptrs.append((i, c & ~1))
                
    print(f"Collected {len(decompressed_ptrs)} tag-3 pointers in shell libapp.so")
    
    # We estimate B as (c_addr - offset) & ~0xffff or & ~0xfff
    for align in [0xfff, 0xffff]:
        candidates = []
        for i, c_addr in decompressed_ptrs:
            b1 = (c_addr - vm_instr_off) & ~align
            if 0 <= (c_addr - b1 - vm_instr_off) < vm_instr_size:
                candidates.append(b1)
                
            b2 = (c_addr - iso_instr_off) & ~align
            if 0 <= (c_addr - b2 - iso_instr_off) < iso_instr_size:
                candidates.append(b2)
                
        counter = Counter(candidates)
        most_common = counter.most_common(5)
        print(f"Alignment {hex(align + 1)} most common bases:")
        for B, count in most_common:
            print(f"  Base {hex(B)}: matched {count} pointers")

if __name__ == '__main__':
    main()
