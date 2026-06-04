import sys
from pathlib import Path

def main():
    engine_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/libflutter_engine.so.bak")
    if not engine_path.exists():
        print("Engine not found")
        return
    
    data = engine_path.read_bytes()
    
    # CodeDeserializationCluster::ReadFill starts at VA 0x2299f40 (file offset 0x2298f40)
    # Let's disassemble 1000 bytes starting from there.
    start_offset = 0x2298f40
    size = 1000
    chunk = data[start_offset:start_offset+size]
    
    try:
        import capstone
        md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
        for i in md.disasm(chunk, start_offset):
            va = i.address - 0x1953248 + 0x1954248
            print(f"File {hex(i.address)} / VA {hex(va)}: {i.mnemonic:<8} {i.op_str:<40} (bytes: {i.bytes.hex()})")
    except ImportError:
        print("Capstone not installed.")

if __name__ == "__main__":
    main()
