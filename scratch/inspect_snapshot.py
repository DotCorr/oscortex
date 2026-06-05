#!/usr/bin/env python3
import sys
from pathlib import Path

def find_symbols(elf_bytes):
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
        return {}
    strtab_hdr = e_shoff + sym_link * e_shentsize
    strtab_off = int.from_bytes(elf_bytes[strtab_hdr+0x18:strtab_hdr+0x20], 'little')
    count = sym_size // sym_entsz
    symbols = {}
    for i in range(count):
        so = sym_off + i * sym_entsz
        st_name = int.from_bytes(elf_bytes[so:so+4], 'little')
        st_value = int.from_bytes(elf_bytes[so+8:so+16], 'little')
        st_size = int.from_bytes(elf_bytes[so+16:so+24], 'little')
        if st_name == 0:
            continue
        curr = strtab_off + st_name
        name_bytes = []
        while curr < len(elf_bytes) and elf_bytes[curr] != 0:
            name_bytes.append(elf_bytes[curr])
            curr += 1
        name = bytes(name_bytes).decode('ascii', errors='ignore')
        if name in ("_kDartVmSnapshotData", "_kDartVmSnapshotInstructions", "_kDartIsolateSnapshotData", "_kDartIsolateSnapshotInstructions"):
            symbols[name] = (st_value, st_size)
    return symbols

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

def analyze():
    libapp_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/flutter/libapp.so")
    if not libapp_path.exists():
        print(f"Error: {libapp_path} does not exist")
        return 1
    
    data = libapp_path.read_bytes()
    symbols = find_symbols(data)

    # Segment sizes and offsets in ELF
    vm_data_off = symbols["_kDartVmSnapshotData"][0]
    vm_data_size = symbols["_kDartVmSnapshotData"][1]
    
    iso_data_off = symbols["_kDartIsolateSnapshotData"][0]
    iso_data_size = symbols["_kDartIsolateSnapshotData"][1]
    
    vm_instr_off = symbols["_kDartVmSnapshotInstructions"][0]
    vm_instr_size = symbols["_kDartVmSnapshotInstructions"][1]
    
    iso_instr_off = symbols["_kDartIsolateSnapshotInstructions"][0]
    iso_instr_size = symbols["_kDartIsolateSnapshotInstructions"][1]

    vm_data_start = vm_data_off
    vm_data_end = vm_data_start + vm_data_size
    
    iso_data_start = iso_data_off
    iso_data_end = iso_data_start + iso_data_size
    
    vm_instr_start = vm_instr_off
    vm_instr_end = vm_instr_start + vm_instr_size
    
    iso_instr_start = iso_instr_off
    iso_instr_end = iso_instr_start + iso_instr_size

    total_limit = max(vm_data_end, iso_data_end, vm_instr_end, iso_instr_end)
    print(f"Total range for pointers: 0 .. 0x{total_limit:x}")

    for name in ("_kDartVmSnapshotData", "_kDartIsolateSnapshotData"):
        if name not in symbols:
            continue
        val, size = symbols[name]
        off = va_to_file_offset(data, val)
        if not off:
            continue
        
        segment_bytes = data[off:off+size]
        
        matches = []
        for i in range(0, size - 4, 4):
            val32 = int.from_bytes(segment_bytes[i:i+4], 'little')
            c = val32 << 1
            if c > 0 and c < total_limit:
                if (c >= vm_data_start and c < vm_data_end) or \
                   (c >= iso_data_start and c < iso_data_end) or \
                   (c >= vm_instr_start and c < vm_instr_end) or \
                   (c >= iso_instr_start and c < iso_instr_end):
                    matches.append((i, val32, c))

        print(f"\n32-bit compressed pointer matches (base 0) in {name}: {len(matches)}")
        for idx, val32, c in matches[:10]:
            print(f"  offset 0x{idx:x}: val=0x{val32:08x} -> c=0x{c:08x}")

if __name__ == "__main__":
    analyze()
