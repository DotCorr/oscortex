from pathlib import Path
import re

def main():
    p = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/lib/libflutter_engine.so")
    if not p.exists():
        print("Engine not found")
        return
    data = p.read_bytes()
    
    # Let's search for "compressed" with all kinds of letters/dashes/underscores
    for term in [b"compressed", b"pointer", b"compress"]:
        matches = []
        for m in re.finditer(term, data, re.IGNORECASE):
            start = max(0, m.start() - 50)
            end = min(len(data), m.end() + 50)
            context = data[start:end]
            # extract printable ascii
            ascii_context = "".join(chr(c) if 32 <= c < 127 else "." for c in context)
            if "compressed-pointers" in ascii_context or "no-compressed-pointers" in ascii_context or "compressed_pointers" in ascii_context or "no_compressed_pointers" in ascii_context:
                print(f"Found match at {m.start()}: {ascii_context}")

if __name__ == "__main__":
    main()
