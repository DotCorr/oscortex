import sys
from pathlib import Path
from elftools.elf.elffile import ELFFile
from elftools.elf.relocation import RelocationSection

def main():
    engine_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/scratch/linux-x64-profile/libflutter_linux_gtk.so")
    if not engine_path.exists():
        print("Engine not found")
        return
    
    with open(engine_path, 'rb') as f:
        elffile = ELFFile(f)
        
        # Print sections containing offset 0xffd650
        for section in elffile.iter_sections():
            sh_addr = section['sh_addr']
            sh_size = section['sh_size']
            sh_offset = section['sh_offset']
            if sh_offset <= 0xffd650 < sh_offset + sh_size:
                print(f"Offset 0xffd650 is in section '{section.name}': addr={hex(sh_addr)}, offset={hex(sh_offset)}, size={hex(sh_size)}")
        
        # Let's inspect the relocations to see if there is any relocation pointing to this address or if it is a PLT slot
        # Usually PLT starts at some address and each entry is 16 bytes.
        # Let's find PLT section
        plt_sec = elffile.get_section_by_name('.plt')
        if plt_sec:
            plt_addr = plt_sec['sh_addr']
            plt_offset = plt_sec['sh_offset']
            plt_size = plt_sec['sh_size']
            print(f".plt section: addr={hex(plt_addr)}, offset={hex(plt_offset)}, size={hex(plt_size)}")
            # Offset 0xffd650 is in memory address 0xffd650 (since virtual address of segment matches offset or is close)
            # Let's see the segment mappings
            for segment in elffile.iter_segments():
                if segment['p_type'] == 'PT_LOAD':
                    p_vaddr = segment['p_vaddr']
                    p_offset = segment['p_offset']
                    p_memsz = segment['p_memsz']
                    if p_offset <= 0xffd650 < p_offset + p_memsz:
                        vaddr = p_vaddr + (0xffd650 - p_offset)
                        print(f"Offset 0xffd650 maps to VA {hex(vaddr)} in PT_LOAD segment with offset {hex(p_offset)}")
        
        # Let's look for relocations (in .rela.dyn or .rela.plt) that correspond to this PLT/GOT entry.
        # Let's scan all relocation sections
        for section in elffile.iter_sections():
            if not isinstance(section, RelocationSection):
                continue
            
            # Let's look for relocations around this virtual address
            # Relocations usually target the GOT (Global Offset Table) entry, which the PLT entry jumps to.
            # Let's find what the PLT entry at 0xffd650 actually does by disassembling the PLT entry.
            # Usually it does: jmp [rip + got_offset]; push ...; jmp ...
            
        # Let's read the 16 bytes at 0xffd650
        f.seek(0xffd650)
        plt_bytes = f.read(16)
        print(f"Bytes at 0xffd650: {plt_bytes.hex()}")
        
        # Let's disassemble these bytes
        import capstone
        md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
        for i in md.disasm(plt_bytes, 0xffd650):
            print(f"  PLT instruction: {i.mnemonic:<8} {i.op_str:<40}")
            
if __name__ == "__main__":
    main()
