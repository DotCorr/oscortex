import subprocess

def main():
    start_va = 0x23db910
    target_start = 0x23dbcc8
    target_end = 0x23dc1b3
    
    elf_path = "tools/flutter-engine/libflutter_engine.so"
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
