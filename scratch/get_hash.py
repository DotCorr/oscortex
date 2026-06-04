import sys
from pathlib import Path

# Add the directory to path
sys.path.append(str(Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine")))
import engine_patch

engine_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/libflutter_engine.so.bak")
h = engine_patch.get_sdk_hash_from_elf(engine_path)
print(f"Hash: {h}")
if h:
    try:
        print(f"Decoded: {h.decode('ascii')}")
    except:
        pass
