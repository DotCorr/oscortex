import sys
from capstone import *

def main():
    path = "/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/libflutter_engine.so.bak"
    with open(path, "rb") as f:
        f.seek(0x22a0200)
        data = f.read(200)
        
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    for i in md.disasm(data, 0x22a0200):
        print(f"0x{i.address:x}:\t{i.mnemonic}\t{i.op_str}")

if __name__ == "__main__":
    main()
