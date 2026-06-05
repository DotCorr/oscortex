import sys
from pathlib import Path
from inspect_snapshot import find_symbols, va_to_file_offset

def main():
    libapp_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/flutter/libapp.so")
    data = libapp_path.read_bytes()
    symbols = find_symbols(data)
    
    vm_data_val = symbols["_kDartVmSnapshotData"][0]
    off = va_to_file_offset(data, vm_data_val)
    
    print(f"VM data symbol val: {hex(vm_data_val)}, file offset: {hex(off)}")
    
    # The stream pointer was at 0x1117939c1.
    # The snapshot base was mapped at 0x111788000.
    # So the offset relative to VM data start is 0x1117939c1 - 0x111788000 = 0xb9c1.
    target_off = off + 0xb9c1
    print(f"Target file offset: {hex(target_off)}")
    
    # Print 32 bytes around target_off
    chunk = data[target_off - 16 : target_off + 32]
    print("Bytes around target offset:")
    for i in range(0, len(chunk), 16):
        off_in_file = target_off - 16 + i
        line = chunk[i:i+16]
        print(f"  {hex(off_in_file)}: {line.hex(' ')}")

if __name__ == "__main__":
    main()
