import struct

def pt_load_span(path):
    with open(path, 'rb') as f:
        elf = f.read()
    
    # Read Elf64Hdr
    # e_phoff is at 32, e_phnum at 56, e_phentsize at 54
    e_phoff = struct.unpack('<Q', elf[32:40])[0]
    e_phentsize = struct.unpack('<H', elf[54:56])[0]
    e_phnum = struct.unpack('<H', elf[56:58])[0]
    
    max_end = 0
    for i in range(e_phnum):
        off = e_phoff + i * e_phentsize
        phdr = elf[off:off+56]
        p_type = struct.unpack('<I', phdr[0:4])[0]
        if p_type != 1: # PT_LOAD
            continue
        p_vaddr = struct.unpack('<Q', phdr[16:24])[0]
        p_memsz = struct.unpack('<Q', phdr[40:48])[0]
        end = p_vaddr + p_memsz
        print(f"Segment {i}: vaddr={hex(p_vaddr)}, memsz={hex(p_memsz)}, end={hex(end)}")
        if end > max_end:
            max_end = end
            
    print("max_end:", hex(max_end))
    stride = 0x0100_0000
    aligned = (max_end + stride - 1) // stride * stride
    print("aligned:", hex(aligned))
    print("next base:", hex(0x0100_0000 + aligned))

pt_load_span('./tools/flutter-engine/libflutter_engine.so')
