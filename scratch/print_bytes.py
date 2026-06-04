import sys
from pathlib import Path

engine_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/libflutter_engine.so")
data = engine_path.read_bytes()

offset = 0x2346f60
print(f"Bytes at offset {hex(offset)}:")
sub = data[offset : offset + 32]
print(" ".join(f"{b:02x}" for b in sub))
