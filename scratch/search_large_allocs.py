import sys
from pathlib import Path

log_path = Path("/private/tmp/osc_serial.log")
lines = log_path.read_text().splitlines()

for idx, line in enumerate(lines):
    if "bump_anon_va" in line:
        # find size=
        try:
            parts = line.split("size=")
            if len(parts) > 1:
                sz_str = parts[1].split()[0]
                sz = int(sz_str, 16)
                if sz >= 0x1000_0000:
                    print(f"Line {idx+1}: {line}")
        except Exception as e:
            pass
