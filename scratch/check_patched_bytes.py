from pathlib import Path

def main():
    p = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/lib/libflutter_engine.so")
    data = p.read_bytes()
    offset = 0x195f260
    bytes_at_offset = data[offset:offset+16]
    print(f"Bytes at offset 0x{offset:x}: {bytes_at_offset.hex()}")

if __name__ == "__main__":
    main()
