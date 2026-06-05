import sys
import re
from pathlib import Path

def main():
    p = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/lib/libflutter_engine.so")
    if not p.exists():
        print("Engine not found")
        return
    data = p.read_bytes()
    print(f"Engine size: {len(data)}")
    
    # Search for a 40-character hex string
    hex_re = re.compile(b"[0-9a-f]{40}")
    matches = hex_re.findall(data)
    if matches:
        print(f"Found {len(matches)} hex strings of length 40:")
        for m in set(matches):
            print(f"  {m.decode('ascii', errors='ignore')}")
            
            # Print feature string surrounding this match
            idx = data.find(m)
            while idx != -1:
                end_idx = data.find(b"\x00", idx)
                if end_idx != -1:
                    feature_str = data[idx:end_idx].decode('ascii', errors='ignore')
                    print(f"    Surrounding feature string: '{feature_str}'")
                idx = data.find(m, idx + 1)
    else:
        print("No 40-character hex strings found")

if __name__ == "__main__":
    main()
