from pathlib import Path

def find_symbol_offset(elf_bytes, sym_name):
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
        if st_name == 0:
            continue
        curr = strtab_off + st_name
        name_bytes = []
        while curr < len(elf_bytes) and elf_bytes[curr] != 0:
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

def main():
    p = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/libflutter_engine.so")
    data = p.read_bytes()
    val = find_symbol_offset(data, "FlutterEngineRunsAOTCompiledDartCode")
    if val is not None:
        print(f"Virtual address: 0x{val:x}")
        fo = va_to_file_offset(data, val)
        print(f"File offset: 0x{fo:x}")
    else:
        print("Symbol not found")

if __name__ == "__main__":
    main()
