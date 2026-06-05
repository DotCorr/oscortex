import struct
import re
from pathlib import Path

def main():
    log_path = Path("/tmp/osc_serial.log")
    engine_path = Path("./initramfs/system/lib/libflutter_engine.so")
    
    if not log_path.exists():
        print(f"Log path {log_path} not found")
        return
    if not engine_path.exists():
        print(f"Engine path {engine_path} not found")
        return
        
    # Find all sym_idx from warnings in serial log
    # Example warning: [WARN] kernel::process::dl: [dl-debug] target=0x141064230 offset=0x1064230 sym_idx=2 value=0x0 rtype=7
    log_content = log_path.read_text(errors='ignore')
    sym_indices = set()
    pattern = re.compile(r"sym_idx=(\d+)")
    for line in log_content.splitlines():
        if "value=0x0" in line:
            m = pattern.search(line)
            if m:
                sym_indices.add(int(m.group(1)))
                
    print(f"Found {len(sym_indices)} unique unresolved symbol indices in log")
    if not sym_indices:
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
    
    unresolved_names = []
    for idx in sorted(sym_indices):
        off = symtab_file + idx * 24
        if off + 24 > len(data):
            print(f"Index {idx} is out of bounds")
            continue
        st_name = struct.unpack_from("<I", data, off)[0]
        st_info = data[off + 4]
        st_shndx = struct.unpack_from("<H", data, off + 6)[0]
        st_value = struct.unpack_from("<Q", data, off + 8)[0]
        
        name_off = strtab_file + st_name
        if name_off >= len(data):
            name = f"<invalid offset {hex(name_off)}>"
        else:
            end_off = data.find(b'\0', name_off)
            if end_off == -1:
                name = data[name_off:].decode(errors='ignore')
            else:
                name = data[name_off:end_off].decode(errors='ignore')
        unresolved_names.append((idx, name, st_info, st_shndx, st_value))
        
    # Sort by name
    unresolved_names.sort(key=lambda x: x[1])
    
    print("\n--- Unresolved Symbols ---")
    for idx, name, st_info, st_shndx, st_value in unresolved_names:
        print(f"Index {idx:4d}: {name} (info={st_info}, shndx={st_shndx}, val={hex(st_value)})")

if __name__ == "__main__":
    main()
