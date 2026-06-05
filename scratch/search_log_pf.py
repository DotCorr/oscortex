import sys
from pathlib import Path

log_path = Path("/private/tmp/osc_serial.log")
lines = log_path.read_text().splitlines()

print(f"Total lines in log: {len(lines)}")
print("Matching PageFault lines:")
for idx, line in enumerate(lines):
    if "PageFault" in line:
        print(f"Line {idx+1}: {line}")
