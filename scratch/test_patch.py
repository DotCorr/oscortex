import struct
from pathlib import Path

def main():
    path = Path("./initramfs/system/flutter/libapp.so")
    data = bytearray(path.read_bytes())
    
    load_base = 0x483000000
    runtime_cage_base = load_base & 0xffff_ffff_0000_0000
    
    # We know from ELF program headers:
    # Segment 1 (Data): offset=0x0, size=0x1cbbb2
    # Segment 2 (Instructions): offset=0x1cc000, size=0x25a9f0
    
    # Let's define the compile-time bases:
    # Segment 1 starts at 0x1_4000_0000
    # Segment 2 base B2 is what we want to find. Let's try 0x1_4f23_4000 as indicated by min_c.
    # Wait, in test_patch.py, min_c for iso_instr was 0x147030396.
    # Wait, the range of isolate instructions was [0x147030396, 0x14ff8ec06].
    # So B2 must be such that 0x147030396 is within B2 + 0x1d2e80.
    # Let's find the exact B2 by matching pointers to known instruction offsets.
    
    # Actually, let's write a loop to test different B2 values and see which one makes
    # decompressed pointer match Segment 2 runtime address.
    # For a pointer c_addr, if it points to Segment 2 (instructions):
    # runtime_address = load_base + 0x1cc000 + (c_addr - B2 - 0x1cc000) = load_base + c_addr - B2.
    # So if B2 is correct, then (c_addr - B2) must be the actual offset in Segment 2.
    # And corrected pointer = (runtime_address - runtime_cage_base) >> 1
    # = (load_base - runtime_cage_base + c_addr - B2) >> 1.
    
    B2_options = [0x1_4f23_4000, 0x1_4f23_8000, 0x1_4f24_0000, 0x1_4f24_5000, 0x1_46e6_0000]
    # Wait, let's look at the min/max of VM instructions: [0x14660f466, 0x1468a05d6].
    # For VM instructions, if they start at B_vm:
    # B_vm = 0x1_4660_0000?
    
    # Let's try to dynamically find B_vm and B_iso for each pointer by assuming:
    # 1. Any pointer in [0x1_4600_0000, 0x1_4690_0000) is VM instructions.
    # 2. Any pointer in [0x1_4700_0000, 0x1_5000_0000) is Isolate instructions.
    # Let's test the bases we got from find_base.py:
    # VM instructions base = 0x1_466c_0000
    # Isolate instructions base = 0x1_47ea_0000 (or others)
    
    print("Testing bases...")
    # Let's analyze a few specific pointers and print their expected runtime offsets.
    count = 0
    for i in range(0, len(data) - 4, 4):
        val = struct.unpack("<I", data[i:i+4])[0]
        if 0x2000_0000 <= val < 0x2800_0000:
            c = 0x1_0000_0000 + (val << 1)
            c_addr = c & ~1
            if (c & 0x7) in (6, 7): # instructions pointer
                count += 1
                if count <= 10:
                    print(f"Instruction ptr #{count} at {hex(i)}: val={hex(val)} c_addr={hex(c_addr)}")
                    
if __name__ == '__main__':
    main()
