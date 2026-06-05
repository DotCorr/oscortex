#!/usr/bin/env python3
import sys
from pathlib import Path

def main():
    if len(sys.argv) < 2:
        print("Usage: patch_libapp.py <path_to_libapp.so>", file=sys.stderr)
        return 1

    libapp_path = Path(sys.argv[1])
    if not libapp_path.exists():
        print(f"Error: {libapp_path} does not exist", file=sys.stderr)
        return 1

    data = bytearray(libapp_path.read_bytes())
    
    # We patch the target OS in the snapshot's features string so the Linux engine accepts it.
    # Both strings are exactly 32 bytes long, ensuring in-place modification without shifting offsets.
    target = b"x64 macos no-compressed-pointers"
    replacement = b"x64 linux no-compressed-pointers"
    
    count = 0
    idx = data.find(target)
    while idx != -1:
        data[idx : idx + len(target)] = replacement
        count += 1
        idx = data.find(target, idx + len(target))
        
    if count > 0:
        libapp_path.write_bytes(data)
        print(f"Patched {count} occurrences of target OS features in {libapp_path.name}")
        return 0
    else:
        print(f"Warning: No occurrences of target OS features found in {libapp_path.name}")
        # Return 0 so build process doesn't fail if already patched
        return 0

if __name__ == "__main__":
    sys.exit(main())
