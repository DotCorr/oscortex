import sys
from pathlib import Path

log_path = Path("/private/tmp/osc_serial.log")
lines = log_path.read_text().splitlines()

for idx, line in enumerate(lines):
    if "resolved via dlsym" in line or "Relocated AOT" in line:
        print(f"Line {idx+1}:")
        for i in range(max(0, idx-2), min(len(lines), idx+10)):
            print(f"  {i+1}: {lines[i]}")
