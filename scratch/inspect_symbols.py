import sys
from pathlib import Path

libapp_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/flutter/libapp.so")
if not libapp_path.exists():
    print("libapp.so not found")
    sys.exit(1)

data = libapp_path.read_bytes()

def print_symbols(elf_bytes):
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
        if sh_type in (2, 11):
            sym_off = int.from_bytes(elf_bytes[off+0x18:off+0x20], 'little')
            sym_size = int.from_bytes(elf_bytes[off+0x20:off+0x28], 'little')
            sym_entsz = int.from_bytes(elf_bytes[off+0x38:off+0x40], 'little')
            sym_link = int.from_bytes(elf_bytes[off+0x28:off+0x2c], 'little')
            
            strtab_hdr = e_shoff + sym_link * e_shentsize
            strtab_off = int.from_bytes(elf_bytes[strtab_hdr+0x18:strtab_hdr+0x20], 'little')
            
            count = sym_size // sym_entsz
            print(f"Section {i} symbol count: {count}")
            for j in range(count):
                so = sym_off + j * sym_entsz
                st_name = int.from_bytes(elf_bytes[so:so+4], 'little')
                st_value = int.from_bytes(elf_bytes[so+8:so+16], 'little')
                st_size = int.from_bytes(elf_bytes[so+16:so+24], 'little')
                
                name_bytes = []
                curr = strtab_off + st_name
                while curr < len(elf_bytes) and elf_bytes[curr] != 0:
                    name_bytes.append(elf_bytes[curr])
                    curr += 1
                name = bytes(name_bytes).decode('ascii', errors='ignore')
                if "kDart" in name:
                    print(f"Symbol: {name} | value: {hex(st_value)} | size: {hex(st_size)}")

print_symbols(data)
