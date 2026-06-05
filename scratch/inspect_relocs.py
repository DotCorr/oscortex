import struct

path = "/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/flutter/libapp.so"

def parse_elf(path):
    with open(path, "rb") as f:
        elf = f.read()
    
    phoff = struct.unpack_from("<Q", elf, 32)[0]
    phentsize = struct.unpack_from("<H", elf, 54)[0]
    phnum = struct.unpack_from("<H", elf, 56)[0]
    
    dynamic_ph = None
    for i in range(phnum):
        off = phoff + i * phentsize
        p_type, p_flags, p_offset, p_vaddr, p_paddr, p_filesz, p_memsz, p_align = struct.unpack_from("<IIQQQQQQ", elf, off)
        if p_type == 2: # PT_DYNAMIC
            dynamic_ph = (p_offset, p_filesz)
            break
            
    if not dynamic_ph:
        print("No PT_DYNAMIC")
        return
        
    offset, size = dynamic_ph
    count = size // 16
    for i in range(count):
        d_tag, d_val = struct.unpack_from("<qQ", elf, offset + i * 16)
        print("Entry {:2d}: tag={}, val=0x{:x}".format(i, d_tag, d_val))

parse_elf(path)
