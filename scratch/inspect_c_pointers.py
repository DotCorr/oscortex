import sys
import struct
from pathlib import Path

libapp_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/flutter/libapp.so")
if not libapp_path.exists():
    print("libapp.so not found")
    sys.exit(1)

data = libapp_path.read_bytes()

# Snapshot symbol values (relative to compile-time base 0x1_4000_0000)
vm_data_c_start = 0x140000000 + 0x340
vm_data_c_end = vm_data_c_start + 0x9830

iso_data_c_start = 0x140000000 + 0x9b80
iso_data_c_end = iso_data_c_start + 0x1c1ff0

vm_instr_c_start = 0x140000000 + 0x1cc000
vm_instr_c_end = vm_instr_c_start + 0x6e70

iso_instr_c_start = 0x140000000 + 0x1d2e80
iso_instr_c_end = iso_instr_c_start + 0x253b70

def is_valid_pointer(c):
    return (vm_data_c_start <= c < vm_data_c_end) or \
           (iso_data_c_start <= c < iso_data_c_end) or \
           (vm_instr_c_start <= c < vm_instr_c_end) or \
           (iso_instr_c_start <= c < iso_instr_c_end)

def get_target_name(c):
    if vm_data_c_start <= c < vm_data_c_end:
        return "vm_data"
    if iso_data_c_start <= c < iso_data_c_end:
        return "iso_data"
    if vm_instr_c_start <= c < vm_instr_c_end:
        return "vm_instr"
    if iso_instr_c_start <= c < iso_instr_c_end:
        return "iso_instr"
    return "unknown"

def scan_segment(name, offset, size):
    print(f"--- Scanning {name} ---")
    valid_count = 0
    invalid_count = 0
    
    for i in range(0, size - 4, 4):
        val = struct.unpack_from("<I", data, offset + i)[0]
        if 0x20000000 <= val < 0x28000000:
            c = 0x100000000 + (val << 1)
            if is_valid_pointer(c):
                if valid_count < 15:
                    print(f"Valid ptr at offset {hex(i)}: {hex(val)} -> {hex(c)} ({get_target_name(c)})")
                valid_count += 1
            else:
                if invalid_count < 5:
                    print(f"INVALID ptr at offset {hex(i)}: {hex(val)} -> {hex(c)}")
                invalid_count += 1
                
    print(f"Total valid: {valid_count}, Total invalid: {invalid_count}")

scan_segment("VM Snapshot Data", 0x340, 0x9830)
scan_segment("Isolate Snapshot Data", 0x9b80, 0x1c1ff0)
