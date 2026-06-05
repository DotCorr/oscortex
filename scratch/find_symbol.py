import subprocess
import sys

def main():
    if len(sys.argv) > 1:
        offset = int(sys.argv[1], 16) if sys.argv[1].startswith("0x") else int(sys.argv[1])
    else:
        offset = 0x23dacc8
    print(f"Searching for symbol spanning offset 0x{offset:x}...")
    
    # Run nm to get all symbols with sizes
    res = subprocess.run(["nm", "-S", "-C", "tools/flutter-engine/libflutter_engine.so"], capture_output=True, text=True)
    if res.returncode != 0:
        print("nm failed:", res.stderr)
        return
        
    candidates = []
    for line in res.stdout.splitlines():
        parts = line.split()
        if len(parts) >= 4:
            try:
                addr = int(parts[0], 16)
                size = int(parts[1], 16)
                sym_type = parts[2]
                name = " ".join(parts[3:])
                if addr <= offset < addr + size:
                    candidates.append((addr, size, name))
            except ValueError:
                continue
        elif len(parts) == 3:
            # No size, just address
            try:
                addr = int(parts[0], 16)
                if addr <= offset < addr + 0x1000: # heuristic fallback
                    candidates.append((addr, 0, parts[2]))
            except ValueError:
                continue
                
    if not candidates:
        # Let's search by closest preceding address
        best = None
        for line in res.stdout.splitlines():
            parts = line.split()
            if len(parts) >= 3:
                try:
                    addr = int(parts[0], 16)
                    if addr <= offset:
                        if best is None or addr > best[0]:
                            best = (addr, " ".join(parts[2:]))
                except ValueError:
                    continue
        if best:
            print(f"Closest preceding symbol: 0x{best[0]:x} -> {best[1]}")
        else:
            print("No symbol found.")
    else:
        for addr, size, name in candidates:
            print(f"Symbol found: 0x{addr:x} (size 0x{size:x}) -> {name}")

if __name__ == "__main__":
    main()
