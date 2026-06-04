import sys
from pathlib import Path

log_path = Path("/private/tmp/osc_serial.log")
lines = log_path.read_text().splitlines()

for idx, line in enumerate(lines):
    if "resolved via dlsym" in line or "content:" in line or "Relocated AOT" in line or "vm_instr" in line:
        print(f"Line {idx+1}: {line}")
