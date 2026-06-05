import subprocess
import sys

def main():
    if len(sys.argv) > 3:
        start_va = int(sys.argv[1], 16) if sys.argv[1].startswith("0x") else int(sys.argv[1])
        target_start = int(sys.argv[2], 16) if sys.argv[2].startswith("0x") else int(sys.argv[2])
        target_end = int(sys.argv[3], 16) if sys.argv[3].startswith("0x") else int(sys.argv[3])
    else:
        start_va = 0x2355ff0
        target_start = 0x2355ff0
        target_end = 0x2356015
        
    elf_path = "tools/flutter-engine/libflutter_engine.so"
        
    print(f"Disassembling {elf_path} aligned from 0x{start_va:x}...")
    
    res = subprocess.run([
        "objdump", "-d", 
        f"--start-address={start_va}",
        f"--stop-address={target_end}",
        elf_path
    ], capture_output=True, text=True)
    
    if res.returncode != 0:
        print("objdump failed:", res.stderr)
        return
        
    printing = False
    for line in res.stdout.splitlines():
        if ":" in line:
            parts = line.split(":")
            try:
                addr = int(parts[0].strip(), 16)
                if target_start <= addr <= target_end:
                    printing = True
                elif addr > target_end:
                    printing = False
            except ValueError:
                pass
        if printing:
            print(line)

if __name__ == "__main__":
    main()
