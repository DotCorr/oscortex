import subprocess
import re

lib_path = "tools/flutter-engine/libflutter_engine.so"
target_addrs = [
    0x19bafc6,
    0x19bafb4,
    0x19bbd0f,
    0x266f840,
    0x266f400,
    0x26f8a48,
    0x183a3f,
    0x196e565,
    0x266e248,
    0x2206060,
    0x26b9b78,
    0x266a308,
    0x2205e4b,
    0x158d5e
]

print("Searching for symbols...")

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
