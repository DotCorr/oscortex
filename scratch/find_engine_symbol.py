import subprocess
from pathlib import Path

engine_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/libflutter_engine.so")

# Run nm on the engine
out = subprocess.check_output(["nm", "-S", str(engine_path)], text=True)

target_offset = 0x2346f60
closest_sym = None
closest_diff = 1000000000

for line in out.splitlines():
    parts = line.split()
    if len(parts) >= 3:
        try:
            val = int(parts[0], 16)
            # check if it's a function or code symbol (usually 't' or 'T')
            t = parts[1] if len(parts) == 3 else parts[2]
            name = parts[-1]
            if val <= target_offset:
                diff = target_offset - val
                if diff < closest_diff:
                    closest_diff = diff
                    closest_sym = (name, val, diff)
        except Exception:
            pass

print(f"Closest symbol to offset {hex(target_offset)}:")
if closest_sym:
    print(f"  Name: {closest_sym[0]}")
    print(f"  Address: {hex(closest_sym[1])}")
    print(f"  Diff: {closest_sym[2]} bytes")
else:
    print("  None found")
