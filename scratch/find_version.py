from pathlib import Path
import re

def main():
    p = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/lib/libflutter_engine.so")
    if not p.exists():
        print("Engine not found")
        return
    data = p.read_bytes()
    
    # Flutter engine version is usually a 40-character commit hash string.
    # It might be in ASCII. Let's look for any 40-character hex string (upper/lower case)
    pat = re.compile(b"[0-9a-fA-F]{40}")
    matches = pat.findall(data)
    print(f"Matches [0-9a-fA-F]{{40}}: {len(matches)}")
    for m in set(matches):
        print(f"  Hex match: {m.decode()}")
        
    # Let's search for common engine version string prefix: "engine" or "version" or "flutter"
    for word in [b"flutter", b"Dart version", b"Engine version"]:
        idx = data.find(word)
        if idx != -1:
            print(f"Found word '{word.decode()}' at {idx}")
            print(f"  Context: {data[idx:idx+128]}")

if __name__ == "__main__":
    main()
