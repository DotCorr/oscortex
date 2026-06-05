import sys
import struct

def main():
    target = 0x19ba2e0
    path = "tools/flutter-engine/libflutter_engine.so"
    
    with open(path, "rb") as f:
        elf = f.read()
        
    # We want to find x86_64 call instructions: 'E8 xx xx xx xx'
    # call offset is relative to the next instruction: PC + 5 + offset = target => offset = target - PC - 5
    for i in range(len(elf) - 5):
        if elf[i] == 0xE8:
            offset = struct.unpack("<i", elf[i+1:i+5])[0]
            pc = i
            dest = (pc + 5 + offset) & 0xffffffffffffffff
            if dest == target:
                print(f"Call to {hex(target)} at file offset {hex(pc)}")

if __name__ == "__main__":
    main()
