import struct
from pathlib import Path

def main():
    elf_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/flutter/libapp.so")
    if not elf_path.exists():
        print("libapp.so not found")
        return
        
    data = elf_path.read_bytes()
    e_phoff = int.from_bytes(data[0x20:0x28], "little")
    e_phnum = int.from_bytes(data[0x38:0x3a], "little")
    e_phentsize = int.from_bytes(data[0x36:0x38], "little")
    
    print(f"Program Headers (count={e_phnum}):")
    for i in range(e_phnum):
        off = e_phoff + i * e_phentsize
        p_type = int.from_bytes(data[off:off+4], "little")
        p_offset = int.from_bytes(data[off+8:off+16], "little")
        p_vaddr = int.from_bytes(data[off+16:off+24], "little")
        p_filesz = int.from_bytes(data[off+32:off+40], "little")
        p_memsz = int.from_bytes(data[off+40:off+48], "little")
        p_flags = int.from_bytes(data[off+4:off+8], "little")
        
        type_str = {1: "PT_LOAD", 2: "PT_DYNAMIC", 3: "PT_INTERP", 4: "PT_NOTE"}.get(p_type, f"TYPE_{p_type}")
        print(f"  {type_str}: file_off=0x{p_offset:x} vaddr=0x{p_vaddr:x} filesz=0x{p_filesz:x} memsz=0x{p_memsz:x} flags={p_flags}")

if __name__ == "__main__":
    main()
