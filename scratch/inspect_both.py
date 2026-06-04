import sys
from pathlib import Path

def inspect_range(data, file_offset, name):
    chunk = data[file_offset:file_offset+32]
    print(f"--- {name} at file offset {hex(file_offset)} ---")
    print("Bytes:", chunk.hex())
    
    import capstone
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    print("Disassembly:")
    for i in md.disasm(chunk, file_offset):
        va = i.address - 0x1953248 + 0x1954248
        print(f"File {hex(i.address)} / VA {hex(va)}: {i.mnemonic:<8} {i.op_str:<40}")

def main():
    engine_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/lib/libflutter_engine.so")
    if not engine_path.exists():
        print("Engine not found")
        return
    
    data = engine_path.read_bytes()
    inspect_range(data, 0x22cde60, "0x22cee60 VA")
    inspect_range(data, 0x22cee60, "0x22cfe60 VA")

if __name__ == "__main__":
    main()
