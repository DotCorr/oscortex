import sys
from pathlib import Path

libapp_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/flutter/libapp.so")
data = libapp_path.read_bytes()

e_phoff = int.from_bytes(data[0x20:0x28], 'little')
e_phnum = int.from_bytes(data[0x38:0x3A], 'little')
e_phentsize = int.from_bytes(data[0x36:0x38], 'little')

print(f"Program headers offset: {e_phoff}, count: {e_phnum}")
for i in range(e_phnum):
    off = e_phoff + i * e_phentsize
    p_type = int.from_bytes(data[off:off+4], 'little')
    p_flags = int.from_bytes(data[off+4:off+8], 'little')
    p_offset = int.from_bytes(data[off+8:off+16], 'little')
    p_vaddr = int.from_bytes(data[off+16:off+24], 'little')
    p_paddr = int.from_bytes(data[off+24:off+32], 'little')
    p_filesz = int.from_bytes(data[off+32:off+40], 'little')
    p_memsz = int.from_bytes(data[off+40:off+48], 'little')
    p_align = int.from_bytes(data[off+48:off+56], 'little')
    print(f"PH[{i}]: type={p_type} flags={p_flags} offset={hex(p_offset)} vaddr={hex(p_vaddr)} filesz={hex(p_filesz)} memsz={hex(p_memsz)} align={hex(p_align)}")
