import sys
from pathlib import Path

def main():
    engine_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/libflutter_engine.so.bak")
    if not engine_path.exists():
        print("Engine not found")
        return
    
    data = engine_path.read_bytes()
    
    # PageFault happened at VA 0x14229a1a8. Relative to base is 0x229a1a8.
    # File offset is 0x229a1a8 - 0x1954248 + 0x1953248 = 0x22991a8.
    target_offset = 0x2298fbd
    start_offset = 0x2298fbd
    end_offset = target_offset + 120
    
    print(f"Target file offset: {hex(target_offset)}")
    print(f"Hex around target ({hex(start_offset)} - {hex(end_offset)}):")
    chunk = data[start_offset:end_offset]
    
    try:
        import capstone
        md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
        for i in md.disasm(chunk, start_offset):
            marker = "  <-- PAGEFAULT IP" if i.address == target_offset else ""
            # Convert file address back to VA for comparison
            va = i.address - 0x1953248 + 0x1954248
            print(f"File {hex(i.address)} / VA {hex(va)}: {i.mnemonic:<8} {i.op_str:<30} (bytes: {i.bytes.hex()}){marker}")
    except ImportError:
        print("Capstone not installed. Hex dump:")
        for idx in range(0, len(chunk), 16):
            off = start_offset + idx
            line = chunk[idx:idx+16]
            print(f"{hex(off)}: {line.hex()}")

if __name__ == "__main__":
    main()
