import sys
from pathlib import Path

def main():
    engine_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/libflutter_engine.so")
    if not engine_path.exists():
        print("Engine not found")
        return
    
    data = engine_path.read_bytes()
    
    # panic IP was 0x142f4c802. Relative to base is 0x1f4c802.
    # File offset = 0x1f4c802 - 0x1954248 + 0x1953248 = 0x1f4b802.
    start_offset = 0x1f4b800
    size = 100
    chunk = data[start_offset:start_offset+size]
    
    import capstone
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    for i in md.disasm(chunk, start_offset):
        va = i.address - 0x1953248 + 0x1954248
        print(f"File {hex(i.address)} / VA {hex(va)}: {i.mnemonic:<8} {i.op_str:<45} (bytes: {i.bytes.hex()})")

if __name__ == "__main__":
    main()
