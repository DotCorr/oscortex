import re
from pathlib import Path

def main():
    p = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/lib/libflutter_engine.so")
    if not p.exists():
        print("Engine not found")
        return
    data = p.read_bytes()
    
    # We can split the binary into null-terminated blocks and filter
    blocks = data.split(b"\x00")
    print(f"Total null-terminated blocks: {len(blocks)}")
    
    # Find blocks containing compressed/pointer/compressed_pointers/compressed-pointers/etc.
    matches = []
    for b in blocks:
        if len(b) > 2:
            bl = b.lower()
            if b"compressed" in bl or b"pointer" in bl:
                try:
                    decoded = b.decode('ascii')
                    if len(decoded) < 150: # filter out long symbol names
                        matches.append(decoded)
                except:
                    pass
    
    print(f"Found {len(matches)} matching strings:")
    for m in sorted(list(set(matches))):
        print(f"  '{m}'")

if __name__ == "__main__":
    main()
