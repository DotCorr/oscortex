import struct
from pathlib import Path
import subprocess

def get_sdk_hash_from_elf(elf_path):
    try:
        out = subprocess.check_output(["nm", "-D", str(elf_path)], text=True)
        for line in out.splitlines():
            if "_kDartVmSnapshotData" in line:
                parts = line.split()
                if len(parts) >= 3:
                    val = int(parts[0], 16)
                    data = elf_path.read_bytes()
                    e_phoff = struct.unpack("<Q", data[0x20:0x28])[0]
                    e_phnum = struct.unpack("<H", data[0x38:0x3a])[0]
                    e_phentsize = struct.unpack("<H", data[0x36:0x38])[0]
                    
                    file_offset = None
                    for i in range(e_phnum):
                        ph_offset = e_phoff + i * e_phentsize
                        p_type = struct.unpack("<I", data[ph_offset : ph_offset + 4])[0]
                        if p_type == 1: # PT_LOAD
                            p_offset = struct.unpack("<Q", data[ph_offset + 8 : ph_offset + 16])[0]
                            p_vaddr = struct.unpack("<Q", data[ph_offset + 16 : ph_offset + 24])[0]
                            p_filesz = struct.unpack("<Q", data[ph_offset + 32 : ph_offset + 40])[0]
                            if val >= p_vaddr and val < p_vaddr + p_filesz:
                                file_offset = p_offset + (val - p_vaddr)
                                break
                    
                    if file_offset is not None:
                        hash_bytes = data[file_offset + 20 : file_offset + 30]
                        return hash_bytes
    except Exception as e:
        print("Error:", e)
    return None

print(get_sdk_hash_from_elf(Path("initramfs/system/flutter/libapp.so")))
