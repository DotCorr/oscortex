from pathlib import Path

def get_symbols(elf_bytes):
    e_shoff = int.from_bytes(elf_bytes[0x28:0x30], 'little')
    e_shentsize = int.from_bytes(elf_bytes[0x3a:0x3c], 'little')
    e_shnum = int.from_bytes(elf_bytes[0x3c:0x3e], 'little')
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
        return []
    strtab_hdr = e_shoff + sym_link * e_shentsize
    strtab_off = int.from_bytes(elf_bytes[strtab_hdr+0x18:strtab_hdr+0x20], 'little')
    count = sym_size // sym_entsz
    syms = []
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
        syms.append((st_value, st_size, name))
    return sorted(syms)

def main():
    p = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/libflutter_engine.so")
    data = p.read_bytes()
    syms = get_symbols(data)
    
    target_va = 0x2292070
    print(f"Target VA: 0x{target_va:x}")
    
    # Find the function containing target_va
    found = False
    for val, size, name in syms:
        if val <= target_va < val + size:
            print(f"Contains target: {name} (start=0x{val:x}, size={size}, end=0x{val+size:x})")
            found = True
            
    if not found:
        # Find nearest symbol before target_va
        closest = None
        for val, size, name in syms:
            if val <= target_va:
                closest = (val, size, name)
        if closest:
            val, size, name = closest
            print(f"Closest before: {name} (start=0x{val:x}, size={size}, end=0x{val+size:x}, diff=0x{target_va - val:x})")

if __name__ == "__main__":
    main()
