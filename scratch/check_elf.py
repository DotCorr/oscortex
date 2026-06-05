import struct

def check_elf(path):
    with open(path, 'rb') as f:
        elf = f.read()
    
    # Read Elf64_Hdr
    # e_ident: 16B, e_type: 2B, e_machine: 2B, e_version: 4B, e_entry: 8B, e_phoff: 8B, e_shoff: 8B, e_flags: 4B, e_ehsize: 2B, e_phentsize: 2B, e_phnum: 2B
    e_phoff = struct.unpack_from('<Q', elf, 32)[0]
    e_phentsize = struct.unpack_from('<H', elf, 54)[0]
    e_phnum = struct.unpack_from('<H', elf, 56)[0]
    print(f"e_phoff: {e_phoff}, e_phentsize: {e_phentsize}, e_phnum: {e_phnum}")
    
    # Read each program header using our struct layout
    # struct Elf64Phdr {
    #     p_type:   u32,
    #     p_flags:  u32,
    #     p_offset: u64,
    #     p_vaddr:  u64,
    #     p_paddr:  u64,
    #     p_filesz: u64,
    #     p_memsz:  u64,
    #     p_align:  u64,
    # }
    for i in range(e_phnum):
        off = e_phoff + i * e_phentsize
        phdr_bytes = elf[off:off+e_phentsize]
        if len(phdr_bytes) < 56:
            break
        p_type, p_flags, p_offset, p_vaddr, p_paddr, p_filesz, p_memsz, p_align = struct.unpack('<IIQQQQQQ', phdr_bytes[:56])
        if p_type == 1: # PT_LOAD
            end = p_vaddr + p_memsz
            flags_str = ""
            if p_flags & 4: flags_str += "R"
            if p_flags & 2: flags_str += "W"
            if p_flags & 1: flags_str += "X"
            print(f"LOAD {i}: p_offset={hex(p_offset)}, p_vaddr={hex(p_vaddr)}, p_memsz={hex(p_memsz)}, flags={flags_str} ({p_flags})")

print("Checking libflutter_engine.so:")
check_elf('initramfs/system/lib/libflutter_engine.so')
print("\nChecking libapp.so:")
check_elf('initramfs/system/flutter/libapp.so')
