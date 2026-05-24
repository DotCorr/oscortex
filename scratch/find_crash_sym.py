import subprocess
import re

lib_path = "tools/flutter-engine/libflutter_engine.so"
target_addrs = [
    0x226ccb4,
    0x231f248,
    0x2486b89,
    0x24864db,
    0x23ee6d9,
    0x251f101,
    0x23b382f,
    0x23f2a18
]

print("Searching for symbols...")

# Run nm -S to get all symbols with sizes
proc = subprocess.Popen(["nm", "-S", lib_path], stdout=subprocess.PIPE, text=True)

found = {}
for line in proc.stdout:
    m = re.match(r'^([0-9a-fA-F]+)\s+([0-9a-fA-F]+)\s+[g|l|t|T]\s+(.*)', line)
    if m:
        addr = int(m.group(1), 16)
        size = int(m.group(2), 16)
        name = m.group(3).strip()
        for target in target_addrs:
            if addr <= target < addr + size:
                found[target] = (name, addr, size)

for target in target_addrs:
    if target in found:
        name, addr, size = found[target]
        print(f"Address {hex(target)} -> {name} (offset={hex(target - addr)})")
    else:
        print(f"Address {hex(target)} -> NOT FOUND")

proc.wait()
