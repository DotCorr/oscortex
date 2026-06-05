import sys

path = "/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/libflutter_engine.so"
start_offset = 0x22952e0
end_offset = 0x2296430

# We don't have capstone installed maybe, so let's just print bytes or check.
# Actually, let's disassemble using objdump in a pipe.
import subprocess

try:
    cmd = ["objdump", "-d", f"--start-address={start_offset}", f"--stop-address={end_offset}", path]
    out = subprocess.check_output(cmd).decode('utf-8')
    for line in out.splitlines():
        if "r12" in line.lower():
            print(line)
except Exception as e:
    print("Error:", e)
