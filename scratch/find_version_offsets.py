import sys
from pathlib import Path

libapp_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/flutter/libapp.so")
data = bytearray(libapp_path.read_bytes())

# Simple ELF parser
def find_symbol_val(elf_bytes, sym_name):
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
            break
    if not sym_off:
        return None
    strtab_hdr = e_shoff + sym_link * e_shentsize
    strtab_off = int.from_bytes(elf_bytes[strtab_hdr+0x18:strtab_hdr+0x20], 'little')
    count = sym_size // sym_entsz
    for i in range(count):
        so = sym_off + i * sym_entsz
        st_name = int.from_bytes(elf_bytes[so:so+4], 'little')
        st_value = int.from_bytes(elf_bytes[so+8:so+16], 'little')
        name_bytes = []
        curr = strtab_off + st_name
        while elf_bytes[curr] != 0:
            name_bytes.append(elf_bytes[curr])
            curr += 1
        name = bytes(name_bytes).decode('ascii', errors='ignore')
        if name == sym_name:
            return st_value
    return None

def va_to_file_offset(elf_bytes, val):
    e_phoff = int.from_bytes(elf_bytes[0x20:0x28], 'little')
    e_phnum = int.from_bytes(elf_bytes[0x38:0x3A], 'little')
    e_phentsize = int.from_bytes(elf_bytes[0x36:0x38], 'little')
    for i in range(e_phnum):
        off = e_phoff + i * e_phentsize
        p_type = int.from_bytes(elf_bytes[off:off+4], 'little')
        if p_type == 1: # PT_LOAD
            p_offset = int.from_bytes(elf_bytes[off+8:off+16], 'little')
            p_vaddr = int.from_bytes(elf_bytes[off+16:off+24], 'little')
            p_filesz = int.from_bytes(elf_bytes[off+32:off+40], 'little')
            if val >= p_vaddr and val < p_vaddr + p_filesz:
                return p_offset + (val - p_vaddr)
    return None

vm_val = find_symbol_val(data, "_kDartVmSnapshotData")
iso_val = find_symbol_val(data, "_kDartIsolateSnapshotData")

vm_off = va_to_file_offset(data, vm_val) if vm_val else None
iso_off = va_to_file_offset(data, iso_val) if iso_val else None

print(f"VM snapshot val: {hex(vm_val) if vm_val else None}, file offset: {hex(vm_off) if vm_off else None}")
print(f"ISO snapshot val: {hex(iso_val) if iso_val else None}, file offset: {hex(iso_off) if iso_off else None}")

if vm_off:
    print(f"VM Magic: {data[vm_off:vm_off+4].hex()}")
    hash_bytes = data[vm_off+20:vm_off+100]
    nul = hash_bytes.find(0)
    print(f"VM Hash: {hash_bytes[:nul].decode('ascii', errors='replace')}")

if iso_off:
    print(f"ISO Magic: {data[iso_off:iso_off+4].hex()}")
    hash_bytes = data[iso_off+20:iso_off+100]
    nul = hash_bytes.find(0)
    print(f"ISO Hash: {hash_bytes[:nul].decode('ascii', errors='replace')}")
