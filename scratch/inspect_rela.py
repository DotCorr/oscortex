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
    DT_JMPREL = 23
    DT_PLTRELSZ = 2
    
    strtab_va = 0
    symtab_va = 0
    jmprel_va = 0
    pltrelsz = 0
    
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
        elif tag == DT_JMPREL:
            jmprel_va = val
        elif tag == DT_PLTRELSZ:
            pltrelsz = val
            
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
    jmprel_file = va_to_file(jmprel_va)
    
    print(f"Jmprel file offset: {hex(jmprel_file)}, size: {pltrelsz}")
    
    # Jmprel relocations (Elf64Rela: r_offset (8B), r_info (8B), r_addend (8B))
    rela_sz = 24
    count = pltrelsz // rela_sz
    for i in range(count):
        off = jmprel_file + i * rela_sz
        r_offset, r_info, r_addend = struct.unpack_from("<QQq", data, off)
        if r_offset == 0x1064ea8:
            sym_idx = r_info >> 32
            r_type = r_info & 0xFFFFFFFF
            print(f"Found relocation for 0x1064ea8 at index {i}:")
            print(f"  r_offset: {hex(r_offset)}")
            print(f"  r_info: {hex(r_info)} (sym_idx={sym_idx}, r_type={r_type})")
            print(f"  r_addend: {r_addend}")
            
            # Look up symbol in symtab
            sym_off = symtab_file + sym_idx * 24
            st_name = struct.unpack_from("<I", data, sym_off)[0]
            st_info = data[sym_off + 4]
            st_shndx = struct.unpack_from("<H", data, sym_off + 6)[0]
            st_value = struct.unpack_from("<Q", data, sym_off + 8)[0]
            name = data[strtab_file + st_name: data.find(b'\0', strtab_file + st_name)].decode()
            print(f"  Symbol index {sym_idx}: {name} st_info={st_info} st_shndx={st_shndx} st_value={hex(st_value)}")

if __name__ == "__main__":
    main()
