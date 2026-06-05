from pathlib import Path
import re

bak_path = Path("tools/flutter-engine/libflutter_engine.so.bak")
data = bak_path.read_bytes()

# Search for the string "DeserializationCluster" or symbols containing it in the binary.
# Since the binary has symbol tables, let's extract them.
import subprocess
try:
    out = subprocess.check_output(["nm", "-C", str(bak_path)], text=True)
    cluster_symbols = []
    for line in out.splitlines():
        if "DeserializationCluster::ReadFill" in line:
            cluster_symbols.append(line)
    
    print(f"Found {len(cluster_symbols)} symbols:")
    for sym in sorted(cluster_symbols):
        print(sym)
except Exception as e:
    print("nm failed, searching via regex in binary data...")
    # Search for ascii strings containing DeserializationCluster or DeserializationRoots
    matches = re.findall(rb"[A-Za-z0-9_:]*(?:DeserializationCluster|DeserializationRoots)[A-Za-z0-9_:]*", data)
    unique_matches = sorted(list(set(m.decode("ascii", errors="ignore") for m in matches)))
    print(f"Found {len(unique_matches)} string matches:")
    for m in unique_matches:
        print(m)
