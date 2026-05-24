import subprocess
import sys

lib_path = "tools/flutter-engine/libflutter_engine.so"
query = sys.argv[1] if len(sys.argv) > 1 else "EnsureInitialized"

print(f"Searching symbols for '{query}'...")
proc = subprocess.Popen(["nm", "-S", lib_path], stdout=subprocess.PIPE, text=True)

for line in proc.stdout:
    if query in line:
        print(line.strip())

proc.wait()
