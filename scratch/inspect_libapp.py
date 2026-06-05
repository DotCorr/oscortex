import sys
from pathlib import Path

# Add the directory to path to use engine_patch if needed, or parse ELF directly.
libapp_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/flutter/libapp.so")
if not libapp_path.exists():
    print("libapp.so not found")
    sys.exit(1)

data = libapp_path.read_bytes()

# Simple ELF parser to find _kDartVmSnapshotData
def find_symbol_val(elf_bytes, sym_name):
    # read headers
    e_shoff = int.from_bytes(elf_bytes[0x28:0x30], 'little')
    e_shentsize = int.from_bytes(elf_bytes[0x3A:0x3C], 'little')
    e_shnum = int.from_bytes(elf_bytes[0x3C:0x3E], 'little')
    
    # find symtab and strtab
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
        
        # read name
        name_bytes = []
        curr = strtab_off + st_name
        while elf_bytes[curr] != 0:
            name_bytes.append(elf_bytes[curr])
            curr += 1
        name = bytes(name_bytes).decode('ascii', errors='ignore')
        if name == sym_name:
            return st_value
    return None

val = find_symbol_val(data, "_kDartVmSnapshotData")
print(f"_kDartVmSnapshotData value: {hex(val) if val else None}")

if val:
    # We need to translate virtual address to file offset.
    # Read program headers.
    e_phoff = int.from_bytes(data[0x20:0x28], 'little')
    e_phnum = int.from_bytes(data[0x38:0x3A], 'little')
    e_phentsize = int.from_bytes(data[0x36:0x38], 'little')
    
    file_offset = None
    for i in range(e_phnum):
        off = e_phoff + i * e_phentsize
        p_type = int.from_bytes(data[off:off+4], 'little')
        if p_type == 1: # PT_LOAD
            p_offset = int.from_bytes(data[off+8:off+16], 'little')
            p_vaddr = int.from_bytes(data[off+16:off+24], 'little')
            p_filesz = int.from_bytes(data[off+32:off+40], 'little')
            if val >= p_vaddr and val < p_vaddr + p_filesz:
                file_offset = p_offset + (val - p_vaddr)
                break
                
    print(f"File offset: {hex(file_offset) if file_offset else None}")
    if file_offset:
        # Read the hash from snapshot header
        # Snapshot header starts with magic 32-bit (0xf5f5dcdc), then features, then version hash.
        magic = data[file_offset : file_offset + 4]
        print(f"Magic: {magic.hex()}")
        # Let's print the ASCII characters starting from file_offset + 20 (where the version string usually is)
        hash_bytes = data[file_offset + 20 : file_offset + 100]
        # find NUL terminator
        nul = hash_bytes.find(0)
        if nul != -1:
            hash_bytes = hash_bytes[:nul]
        print(f"Version Hash in snapshot: {hash_bytes.decode('ascii', errors='replace')}")
