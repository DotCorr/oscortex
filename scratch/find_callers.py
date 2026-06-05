import struct
from pathlib import Path

def main():
    engine_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/libflutter_engine.so.bak")
    if not engine_path.exists():
        print("Engine not found")
        return
    
    data = engine_path.read_bytes()
    
    # Target VA is 0x2292070.
    # Target file offset is 0x2291070.
    target_offset = 0x228fef0
    
    # We search the whole data for call instruction: 0xE8 <disp32>
    # where target_offset = call_instruction_address + 5 + disp32
    # So disp32 = target_offset - call_instruction_address - 5
    
    for i in range(len(data) - 5):
        if data[i] == 0xe8:
            disp = struct.unpack("<i", data[i+1:i+5])[0]
            dest = i + 5 + disp
            if dest == target_offset:
                va = i - 0x1953248 + 0x1954248
                dest_va = dest - 0x1953248 + 0x1954248
                print(f"Call to {hex(dest_va)} at file offset {hex(i)} / VA {hex(va)}")
                
        if data[i] == 0xe9:
            disp = struct.unpack("<i", data[i+1:i+5])[0]
            dest = i + 5 + disp
            if dest == target_offset:
                va = i - 0x1953248 + 0x1954248
                dest_va = dest - 0x1953248 + 0x1954248
                print(f"Jmp to {hex(dest_va)} at file offset {hex(i)} / VA {hex(va)}")

if __name__ == "__main__":
    main()
