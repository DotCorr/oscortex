from pathlib import Path

def main():
    p = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/lib/libflutter_engine.so")
    if not p.exists():
        print("Engine not found")
        return
    data = p.read_bytes()
    
    for term in [b"Dart SDK", b"3.11.0", b"3.11"]:
        idx = data.find(term)
        if idx != -1:
            print(f"Found '{term.decode()}' at {idx}")
            print(f"  Context: {data[idx:idx+128]}")
        else:
            print(f"'{term.decode()}' not found")

if __name__ == "__main__":
    main()
