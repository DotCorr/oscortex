import struct
from pathlib import Path

def main():
    path = Path("./initramfs/system/flutter/libapp.so")
    if not path.exists():
        print("libapp.so not found")
        return
    data = path.read_bytes()
    
    e_phoff = struct.unpack("<Q", data[0x20:0x28])[0]
    e_phnum = struct.unpack("<H", data[0x38:0x3a])[0]
    e_phentsize = struct.unpack("<H", data[0x36:0x38])[0]
    
    print(f"Program headers offset: {e_phoff}, count: {e_phnum}, size: {e_phentsize}")
    for i in range(e_phnum):
        off = e_phoff + i * e_phentsize
        p_type = struct.unpack("<I", data[off:off+4])[0]
        if p_type == 1: # PT_LOAD
            p_flags = struct.unpack("<I", data[off+4:off+8])[0]
            p_offset = struct.unpack("<Q", data[off+8:off+16])[0]
            p_vaddr = struct.unpack("<Q", data[off+16:off+24])[0]
            p_paddr = struct.unpack("<Q", data[off+24:off+32])[0]
            p_filesz = struct.unpack("<Q", data[off+32:off+40])[0]
            p_memsz = struct.unpack("<Q", data[off+40:off+48])[0]
            p_align = struct.unpack("<Q", data[off+48:off+56])[0]
            
            flags_str = ""
            if p_flags & 4: flags_str += "R"
            if p_flags & 2: flags_str += "W"
            if p_flags & 1: flags_str += "X"
            
            print(f"PT_LOAD: offset={hex(p_offset)} vaddr={hex(p_vaddr)} filesz={hex(p_filesz)} memsz={hex(p_memsz)} flags={flags_str}")

if __name__ == '__main__':
    main()
