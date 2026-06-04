import sys
from pathlib import Path

log_path = Path("scratch/osc_serial.log")
if not log_path.exists():
    print("Log not found")
    sys.exit(1)

lines = log_path.read_text(errors='replace').splitlines()

query = sys.argv[1] if len(sys.argv) > 1 else ""
case_insensitive = True

print(f"Searching for '{query}' (case_insensitive={case_insensitive}):")
matches = 0
for idx, line in enumerate(lines):
    match = False
    if case_insensitive:
        match = query.lower() in line.lower()
    else:
        match = query in line
        
    if match:
        print(f"{idx+1}: {line}")
        matches += 1
        if matches >= 100:
            print("Truncated at 100 matches")
            break
            
print(f"Total matches: {matches}")
