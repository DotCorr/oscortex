import sys
from pathlib import Path

libapp_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/flutter/libapp.so")
data = libapp_path.read_bytes()

# Simple ELF symbol parser
def get_symbols(elf_bytes):
    e_shoff = int.from_bytes(elf_bytes[0x28:0x30], 'little')
    e_shentsize = int.from_bytes(elf_bytes[0x3A:0x3C], 'little')
    e_shnum = int.from_bytes(elf_bytes[0x3C:0x3E], 'little')
    
    sym_off = 0
    sym_size = 0
    sym_entsz = 0
    sym_link = 0
    
    for i in range(e_shnum):
        off = e_shoff + i * e_shentsize
        sh_type = int.from_bytes(elf_bytes[off+4:off+8], 'little')
        if sh_type in (2, 11): # SHT_SYMTAB, SHT_DYNSYM
            sym_off = int.from_bytes(elf_bytes[off+0x18:off+0x20], 'little')
            sym_size = int.from_bytes(elf_bytes[off+0x20:off+0x28], 'little')
            sym_entsz = int.from_bytes(elf_bytes[off+0x38:off+0x40], 'little')
            sym_link = int.from_bytes(elf_bytes[off+0x28:off+0x2c], 'little')
            
            strtab_hdr = e_shoff + sym_link * e_shentsize
            strtab_off = int.from_bytes(elf_bytes[strtab_hdr+0x18:strtab_hdr+0x20], 'little')
            
            count = sym_size // sym_entsz
            for j in range(count):
                so = sym_off + j * sym_entsz
                st_name = int.from_bytes(elf_bytes[so:so+4], 'little')
                st_value = int.from_bytes(elf_bytes[so+8:so+16], 'little')
                st_size = int.from_bytes(elf_bytes[so+16:so+24], 'little')
                
                # read name
                name_bytes = []
                curr = strtab_off + st_name
                while elf_bytes[curr] != 0:
                    name_bytes.append(elf_bytes[curr])
                    curr += 1
                name = bytes(name_bytes).decode('ascii', errors='ignore')
                yield name, st_value, st_size

gap_start = 0x352232
gap_end = 0x354000

print("Symbols in gap:")
found = 0
for name, val, size in get_symbols(data):
    if val >= gap_start and val < gap_end:
        print(f"  {name}: value={hex(val)} size={size}")
        found += 1

print(f"Total symbols in gap: {found}")
