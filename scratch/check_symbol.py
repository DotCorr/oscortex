from pathlib import Path

def main():
    p = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/initramfs/system/lib/libflutter_engine.so")
    if not p.exists():
        print("Engine not found")
        return
    data = p.read_bytes()
    
    for s in [b"FlutterEngineRunsAOTCompiledDartCode", b"RunsAOT"]:
        idx = data.find(s)
        if idx != -1:
            print(f"Found '{s.decode()}' at {idx}")
        else:
            print(f"'{s.decode()}' not found")

if __name__ == "__main__":
    main()
