import struct
import sys
from pathlib import Path

def main():
    engine_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/libflutter_engine.so")
    if not engine_path.exists():
        print("Engine not found")
        return 1
    
    data = engine_path.read_bytes()
    target_offset = 1767599 # 0x1af4af
    print(f"Searching for RIP-relative references to target offset 0x{target_offset:x} in RX segment...")
    
    start = 0x1953248
    end = start + 0xd177e8
    
    matches = []
    for p in range(start, end - 4):
        v = struct.unpack("<i", data[p:p+4])[0]
        if p + 0x1004 + v == target_offset:
            opcode_start = max(start, p - 4)
            prefix = data[opcode_start:p]
            print(f"Match found at file offset 0x{p:x} (VA: 0x{p+0x1000:x})")
            print(f"  Instruction opcode start: 0x{opcode_start:x}")
            print(f"  Preceding bytes: {prefix.hex()} [field: {data[p:p+4].hex()}]")
            ctx_start = max(start, p - 20)
            ctx_end = min(end, p + 24)
            print(f"  Context around 0x{p:x}:")
            for i in range(ctx_start, ctx_end, 16):
                chunk = data[i:i+16]
                print(f"    0x{i:x}: {chunk.hex()}")
            matches.append(p)
            
if __name__ == "__main__":
    sys.exit(main())
