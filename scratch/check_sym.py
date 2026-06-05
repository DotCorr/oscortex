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
            
    DT_STRTAB = 5
    DT_SYMTAB = 6
    
    strtab_va = 0
    symtab_va = 0
    
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
    
    target_idxs = [478, 479, 480, 481, 603, 612]
    for idx in target_idxs:
        off = symtab_file + idx * 24
        st_name = struct.unpack_from("<I", data, off)[0]
        st_info = data[off + 4]
        st_shndx = struct.unpack_from("<H", data, off + 6)[0]
        st_value = struct.unpack_from("<Q", data, off + 8)[0]
        name = data[strtab_file + st_name: data.find(b'\0', strtab_file + st_name)].decode()
        print(f"Index {idx}: {name} st_info={st_info} st_shndx={st_shndx} st_value={hex(st_value)}")

if __name__ == "__main__":
    main()
