import struct
from pathlib import Path

def main():
    path = Path("./initramfs/system/flutter/libapp.so")
    data = path.read_bytes()
    
    vm_instr_off = 0x1cc000
    iso_instr_off = 0x1d2e80
    vm_instr_size = 0x6e70
    iso_instr_size = 0x253b70
    
    # We want to identify the exact B for VM and Isolate instructions
    vm_c_vals = []
    iso_c_vals = []
    
    # Let's collect all tag-3 pointers
    for i in range(0, len(data) - 4, 4):
        val = struct.unpack("<I", data[i:i+4])[0]
        if 0x2000_0000 <= val < 0x2800_0000:
            c = 0x1_0000_0000 + (val << 1)
            c_addr = c & ~1
            if (c & 0x7) in (6, 7):
                # Check if it fits the VM instruction segment compile-time space
                # We know the base is around 0x1_466c_0000 or 0x1_466d_0000
                if 0x1_4660_0000 <= c_addr < 0x1_468f_0000:
                    vm_c_vals.append(c_addr)
                # Check if it fits the Isolate instruction segment compile-time space
                # We know the base is around 0x1_47ea_0000 or 0x1_47fb_0000 or 0x1_4f40_0000
                elif 0x1_4700_0000 <= c_addr < 0x1_5000_0000:
                    iso_c_vals.append(c_addr)
                    
    print(f"VM code pointers: count={len(vm_c_vals)}")
    if vm_c_vals:
        print(f"  Min: {hex(min(vm_c_vals))}")
        print(f"  Max: {hex(max(vm_c_vals))}")
        
    print(f"Isolate code pointers: count={len(iso_c_vals)}")
    if iso_c_vals:
        print(f"  Min: {hex(min(iso_c_vals))}")
        print(f"  Max: {hex(max(iso_c_vals))}")

if __name__ == '__main__':
    main()
