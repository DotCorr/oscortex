import capstone
import sys

def main():
    path = "initramfs/system/lib/libflutter_engine.so"
    # VA of crash: 0x2346f60
    # Let's map VA to file offset
    # LOAD 2: p_offset=0x1953248, p_vaddr=0x1954248
    va = 0x2346f60
    file_off = va - 0x1954248 + 0x1953248
    
    with open(path, "rb") as f:
        f.seek(file_off - 32)
        code = f.read(64)
        
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    print("Disassembly around VA 0x{:x} (file offset 0x{:x}):".format(va, file_off))
    for insn in md.disasm(code, va - 32):
        marker = "=>" if insn.address == va else "  "
        print("{} 0x{:x}: {}\t{}".format(marker, insn.address, insn.mnemonic, insn.op_str))

if __name__ == "__main__":
    main()
