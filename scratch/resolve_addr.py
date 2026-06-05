import sys

targets = [0x195f260]

with open("nm_out.txt", "r") as f:
    lines = f.readlines()

for target in targets:
    closest_name = None
    closest_diff = 0xffffffff
    closest_addr = 0
    for line in lines:
        parts = line.strip().split()
        if len(parts) >= 3:
            try:
                addr = int(parts[0], 16)
                diff = target - addr
                if diff >= 0 and diff < closest_diff:
                    closest_diff = diff
                    closest_addr = addr
                    closest_name = parts[2]
            except ValueError:
                continue
    print(f"Target 0x{target:x} -> Closest: {closest_name} at 0x{closest_addr:x} (diff: {closest_diff})")
