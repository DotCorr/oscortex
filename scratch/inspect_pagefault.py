import sys
from pathlib import Path

def main():
    p = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/libflutter_engine.so")
    data = p.read_bytes()
    
    # 0x229a1a8 is the VA.
    # File offset = 0x22991a8
    fo = 0x22991a8
    
    print(f"Bytes at file offset 0x{fo:x}:")
    chunk = data[fo - 32 : fo + 32]
    # Highlight the exact byte at fo
    before = chunk[:32]
    after = chunk[32:]
    print("Before:", before.hex(" "))
    print("Exact at fo:", data[fo:fo+16].hex(" "))
    
    # Disassemble using a simple tool if possible, or just print hex
    print("Whole chunk around fo:")
    for offset in range(fo - 16, fo + 32, 16):
        print(f"0x{offset:x}: {data[offset:offset+16].hex(' ')}")

if __name__ == "__main__":
    main()
