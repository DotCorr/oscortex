import sys
from pathlib import Path

def main():
    engine_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/libflutter_engine.so")
    if not engine_path.exists():
        print("Engine not found")
        return
    
    data = engine_path.read_bytes()
    
    # We want to see from 0x22992b0 to 0x22994f0.
    start_offset = 0x22992b0
    end_offset = 0x22994f0
    chunk = data[start_offset:end_offset]
    
    import capstone
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    for i in md.disasm(chunk, start_offset):
        va = i.address - 0x1953248 + 0x1954248
        if "mov" in i.mnemonic and "r12" in i.op_str and "+" in i.op_str:
            print(f"File {hex(i.address)} / VA {hex(va)}: {i.mnemonic:<8} {i.op_str:<40} (bytes: {i.bytes.hex()})")
        else:
            # Also print the address and instructions around the writes for context
            print(f"  {hex(i.address)} / {hex(va)}: {i.mnemonic:<8} {i.op_str:<40}")

if __name__ == "__main__":
    main()
