import sys

def main():
    path = "/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/libflutter_engine.so.bak"
    with open(path, "rb") as f:
        f.seek(0x2291070)
        data = f.read(500)
    print(data.hex())

if __name__ == "__main__":
    main()
