import capstone

def main():
    path = "initramfs/system/lib/libflutter_engine.so"
    va = 0x2299fbd
    file_off = va - 0x1954248 + 0x1953248
    
    with open(path, "rb") as f:
        f.seek(file_off - 64)
        code = f.read(128)
        
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    print("Disassembly around caller VA 0x{:x}:".format(va))
    for insn in md.disasm(code, va - 64):
        marker = "=>" if insn.address == va else "  "
        print("{} 0x{:x}: {}\t{}".format(marker, insn.address, insn.mnemonic, insn.op_str))

if __name__ == "__main__":
    main()
