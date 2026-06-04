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
        if name in ("_kDartVmSnapshotInstructions", "_kDartIsolateSnapshotInstructions"):
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

def main():
    libapp_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/flutter/libapp.so")
    data = libapp_path.read_bytes()
    symbols = find_symbols(data)
    
    val, size = symbols["_kDartVmSnapshotInstructions"]
    off = va_to_file_offset(data, val)
    
    cursor = off + 0x40
    end = off + size
    
    # Let's print the first 10 instructions objects we find
    count = 0
    while cursor < end and count < 10:
        header = int.from_bytes(data[cursor:cursor+8], 'little')
        if header == 0:
            # Padding
            cursor += 8
            continue
        
        # In Dart 3.x, the UntaggedObject tags format:
        # bit 0: old/new
        # bit 1-7: reserved
        # bit 8-23: class id
        # bit 24-31: size tag
        cid = (header >> 8) & 0xffff
        size_tag = (header >> 24) & 0xff
        
        # If size_tag is 0, the real size is stored at offset 8? Or is it?
        # Let's print the headers
        print(f"Instructions {count}: file_off={hex(cursor)} header={hex(header)} (cid={cid}, size_tag={size_tag})")
        
        # Actually, let's look at the size in bytes of this Instructions object.
        # In AOT, the instructions are padded to 16/32 bytes.
        # Let's check if the size is stored at offset 8 or if we can infer it.
        # Usually, the size is size_tag * 8 or size_tag * 16.
        # If size_tag == 0, the actual size of the object is stored as a 64-bit integer at offset 8 (or offset 16).
        # Let's see what is at offset 8:
        val8 = int.from_bytes(data[cursor+8:cursor+16], 'little')
        val16 = int.from_bytes(data[cursor+16:cursor+24], 'little')
        print(f"  off+8: {hex(val8)}  off+16: {hex(val16)}")
        
        # Let's advance cursor by 64 bytes to see if we hit next header
        cursor += 64
        count += 1

if __name__ == "__main__":
    main()
