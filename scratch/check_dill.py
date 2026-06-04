import sys
from pathlib import Path

def main():
    p = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/apps/oscortex_app/build/app_aot.dill")
    if not p.exists():
        print("Dill file not found")
        return
    data = p.read_bytes()
    print(f"Dill size: {len(data)}")
    
    # Search for VM flag string or use_compressed_pointers
    for s in [b"use_compressed_pointers", b"use-compressed-pointers", b"compressed"]:
        idx = 0
        while True:
            idx = data.find(s, idx)
            if idx == -1:
                break
            print(f"Found {s} at offset {idx}")
            # Print surrounding bytes
            start = max(0, idx - 64)
            end = min(len(data), idx + len(s) + 64)
            surr = data[start:end]
            print(f"  Surrounding ascii: {surr.decode('ascii', errors='ignore')}")
            print(f"  Surrounding bytes: {surr.hex()}")
            idx += 1

if __name__ == "__main__":
    main()
