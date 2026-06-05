import sys
from pathlib import Path

def main():
    engine_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/lib/libflutter_engine.so")
    if not engine_path.exists():
        print("Engine not found")
        return
    
    data = engine_path.read_bytes()
    
    # GetInstructionsAt starts at file offset 0x22cde60 (VA 0x22cee60)
    start_offset = 0x22cde60
    size = 32
    chunk = data[start_offset:start_offset+size]
    
    print("Bytes at 0x22cde60:")
    print(chunk.hex())
    
    import capstone
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    print("Capstone Disassembly:")
    for i in md.disasm(chunk, start_offset):
        va = i.address - 0x1953248 + 0x1954248
        print(f"File {hex(i.address)} / VA {hex(va)}: {i.mnemonic:<8} {i.op_str:<40}")

if __name__ == "__main__":
    main()
