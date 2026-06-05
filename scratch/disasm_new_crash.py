import sys
from pathlib import Path
import capstone

def main():
    engine_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/libflutter_engine.so")
    if not engine_path.exists():
        print("Engine not found")
        return
    
    data = engine_path.read_bytes()
    
    # 0x22a146b is the crash offset relative to load_base.
    # File offset = 0x22a146b - 0x1954248 + 0x1953248 = 0x22a046b
    start_offset = 0x22a0450
    size = 100
    chunk = data[start_offset:start_offset+size]
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    for i in md.disasm(chunk, start_offset):
        va = i.address - 0x1953248 + 0x1954248
        marker = "  <-- CRASH" if i.address == 0x22a046b else ""
        print(f"File {hex(i.address)} / VA {hex(va)}: {i.mnemonic:<8} {i.op_str:<40} (bytes: {i.bytes.hex()}){marker}")

if __name__ == "__main__":
    main()
