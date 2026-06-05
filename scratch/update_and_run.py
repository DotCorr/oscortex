import subprocess
from pathlib import Path

def main():
    # 1. Run assemble_debug_read_instr.py to generate scratch/debug_cave_bytes.bin
    print("Compiling assembly cave...")
    res = subprocess.run(["python3", "scratch/assemble_debug_read_instr.py"], capture_output=True, text=True)
    if res.returncode != 0:
        print("Assembly failed:", res.stderr)
        return
    print(res.stdout)
    
    # 2. Read scratch/debug_cave_bytes.bin
    cave_bin = Path("scratch/debug_cave_bytes.bin").read_bytes()
    hex_str = cave_bin.hex()
    
    # 3. Update P_DEBUG_CAVE in tools/flutter-engine/engine_patch.py
    patch_file = Path("tools/flutter-engine/engine_patch.py")
    content = patch_file.read_text()
    
    # Find P_DEBUG_CAVE line
    start_idx = content.find('"P_DEBUG_CAVE": (0x1F44A00, bytes.fromhex("')
    if start_idx == -1:
        # Try single quotes
        start_idx = content.find("'P_DEBUG_CAVE': (0x1F44A00, bytes.fromhex(\"")
    if start_idx == -1:
        print("Could not find P_DEBUG_CAVE in engine_patch.py")
        return
        
    end_idx = content.find('")),', start_idx)
    if end_idx == -1:
        print("Could not find end of P_DEBUG_CAVE bytes")
        return
        
    prefix = content[:start_idx]
    # Keep the structure: "P_DEBUG_CAVE": (0x1F44A00, bytes.fromhex("<hex>")),
    # We need to construct the new line
    middle = f'"P_DEBUG_CAVE": (0x1F44A00, bytes.fromhex("{hex_str}"))'
    suffix = content[end_idx + 3:]
    
    patch_file.write_text(prefix + middle + suffix)
    print("Updated engine_patch.py with new P_DEBUG_CAVE.")
    
    # 4. Apply engine patches
    print("Applying engine patches...")
    res = subprocess.run(["python3", "tools/flutter-engine/engine_patch.py", "--apply-all"], capture_output=True, text=True)
    if res.returncode != 0:
        print("Engine patch failed:", res.stderr)
        return
    print(res.stdout)
    
    # 5. Build and run ISO
    print("Building and running ISO...")
    # We will invoke build-iso.sh --run
    # Since this starts QEMU and is long-running, we let the caller run it or run it here.
    
if __name__ == "__main__":
    main()
