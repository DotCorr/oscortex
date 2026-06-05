import struct
from pathlib import Path

def main():
    path = Path("initramfs/system/flutter/libapp.so")
    if not path.exists():
        print("libapp.so not found")
        return
    data = path.read_bytes()
    
    vm_instr_off = 0x1cc000
    iso_instr_off = 0x1d2e80
    vm_instr_size = 0x6e70
    iso_instr_size = 0x253b70
    
    instr_start = vm_instr_off
    instr_end = iso_instr_off + iso_instr_size
    
    print(f"Scanning for 64-bit integers in compile-time range [{hex(instr_start)}, {hex(instr_end)})")
    
    matches = []
    for i in range(0, len(data) - 8, 8):
        val = struct.unpack("<Q", data[i:i+8])[0]
        if instr_start <= val < instr_end:
            # Check if it is tagged (HeapObject tag is 1, so val should end in 1, 3, 5, 7, 9, a, b, c, d, e, f? No, tag is 1 so it must end in 1, 3, 5, 7, 9, b, d, f)
            # Actually, let us just check if it is a tagged pointer (ends in 1, 3, 5, etc. or ends in 1, or has instructions tag).
            # In 64-bit Dart VM, instruction pointers have HeapObject tag 1, but wait!
            # Code objects contain pointers to instruction payload which is at offset 16 from Instructions start.
            # But the Instructions pointer itself is tagged 1.
            matches.append((i, val))
            
    print(f"Found {len(matches)} matches")
    print("First 30 matches:")
    for offset, val in matches[:30]:
        # Identify if it points to vm_instr or iso_instr
        seg = "VM" if (vm_instr_off <= val < vm_instr_off + vm_instr_size) else "ISO"
        rel = val - (vm_instr_off if seg == "VM" else iso_instr_off)
        print(f"  File Offset {hex(offset)}: val={hex(val)} ({seg} + {hex(rel)})")

if __name__ == '__main__':
    main()
