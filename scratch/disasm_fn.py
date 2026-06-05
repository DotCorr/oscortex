import sys
from pathlib import Path
import capstone

def main():
    engine_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/scratch/linux-x64-profile/libflutter_linux_gtk.so")
    if not engine_path.exists():
        print("Engine not found")
        return
    
    data = engine_path.read_bytes()
    
    # fl_method_channel_invoke_method is at file offset 0x4abd90 (vaddr 0x4ecd90)
    start_offset = 0x4abd90
    size = 0x60
    chunk = data[start_offset:start_offset+size]
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    for i in md.disasm(chunk, start_offset):
        va = i.address - 0x4abd90 + 0x1404ecd90
        print(f"File {hex(i.address)} / VA {hex(va)}: {i.mnemonic:<8} {i.op_str:<40} (bytes: {i.bytes.hex()})")

if __name__ == "__main__":
    main()
