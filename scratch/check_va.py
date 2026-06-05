import struct
from pathlib import Path

def main():
    engine_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/scratch/linux-x64-profile/libflutter_linux_gtk.so")
    if not engine_path.exists():
        print("Engine not found")
        return
    
    data = engine_path.read_bytes()
    
    phoff = struct.unpack_from("<Q", data, 32)[0]
    phnum = struct.unpack_from("<H", data, 56)[0]
    phentsize = struct.unpack_from("<H", data, 54)[0]
    
    dyn_offset = 0
    dyn_size = 0
    for i in range(phnum):
        off = phoff + i * phentsize
        p_type = struct.unpack_from("<I", data, off)[0]
        if p_type == 2: # PT_DYNAMIC
            dyn_offset = struct.unpack_from("<Q", data, off + 8)[0]
            dyn_size = struct.unpack_from("<Q", data, off + 32)[0]
            break
            
    # Parse dynamic tags
    DT_STRTAB = 5
    DT_SYMTAB = 6
    DT_RELA = 7
    DT_RELASZ = 8
    DT_SYMENT = 11
    
    strtab_va = 0
    symtab_va = 0
    rela_va = 0
    relasz = 0
    syment = 24
    
    ent_sz = 16
    for i in range(dyn_size // ent_sz):
        off = dyn_offset + i * ent_sz
        tag, val = struct.unpack_from("<qQ", data, off)
        if tag == 0:
            break
        elif tag == DT_STRTAB:
            strtab_va = val
        elif tag == DT_SYMTAB:
            symtab_va = val
        elif tag == DT_RELA:
            rela_va = val
        elif tag == DT_RELASZ:
            relasz = val
            
    # Resolve file offsets
    def va_to_file(va):
        for i in range(phnum):
            off = phoff + i * phentsize
            p_type = struct.unpack_from("<I", data, off)[0]
            if p_type == 1: # PT_LOAD
                p_vaddr = struct.unpack_from("<Q", data, off + 16)[0]
                p_offset = struct.unpack_from("<Q", data, off + 8)[0]
                p_filesz = struct.unpack_from("<Q", data, off + 32)[0]
                if p_vaddr <= va < p_vaddr + p_filesz:
                    return p_offset + (va - p_vaddr)
        return None
        
    strtab_file = va_to_file(strtab_va)
    symtab_file = va_to_file(symtab_va)
    rela_file = va_to_file(rela_va)
    
    print(f"Rela file offset: {hex(rela_file)}, size: {relasz}")
    
    # Rela relocations (Elf64Rela: r_offset (8B), r_info (8B), r_addend (8B))
    rela_sz = 24
    count = relasz // rela_sz
    
    target_start = 0x101d998
    target_end = 0x101da18
    
    for i in range(count):
        off = rela_file + i * rela_sz
        r_offset, r_info, r_addend = struct.unpack_from("<QQq", data, off)
        if target_start <= r_offset < target_end:
            sym_idx = r_info >> 32
            r_type = r_info & 0xFFFFFFFF
            print(f"Reloc at {hex(r_offset)} (offset from target_start = +{r_offset - target_start}):")
            print(f"  r_info: {hex(r_info)} (sym_idx={sym_idx}, r_type={r_type})")
            print(f"  r_addend: {hex(r_addend)}")
            if sym_idx != 0:
                sym_off = symtab_file + sym_idx * 24
                st_name = struct.unpack_from("<I", data, sym_off)[0]
                st_value = struct.unpack_from("<Q", data, sym_off + 8)[0]
                name = data[strtab_file + st_name: data.find(b'\0', strtab_file + st_name)].decode()
                print(f"  Symbol: {name} (val={hex(st_value)})")

if __name__ == "__main__":
    main()
