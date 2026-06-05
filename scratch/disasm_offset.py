import sys
from pathlib import Path
import capstone

def main():
    engine_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/scratch/linux-x64-profile/libflutter_linux_gtk.so")
    if not engine_path.exists():
        print("Engine not found")
        return
    
    data = engine_path.read_bytes()
    
    target = bytes.fromhex("89064885ff7508e839908200")
    idx = data.find(target)
    if idx == -1:
        # Let's search for a shorter sequence
        target = bytes.fromhex("89064885ff7508e8")
        idx = data.find(target)
        if idx == -1:
            print("Target bytes not found")
            return
        print(f"Found shorter target at file offset: {hex(idx)}")
    else:
        print(f"Found exact target at file offset: {hex(idx)}")
        
    # Disassemble around the found offset
    start_offset = idx - 10
    size = 40
    chunk = data[start_offset:start_offset+size]
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    for i in md.disasm(chunk, start_offset):
        va = i.address + 0x140000000
        print(f"File {hex(i.address)} / VA {hex(va)}: {i.mnemonic:<8} {i.op_str:<40} (bytes: {i.bytes.hex()})")

if __name__ == "__main__":
    main()
