import re

lib_path = "tools/flutter-engine/libflutter_engine.so"

with open(lib_path, "rb") as f:
    data = f.read()

# Let's search for "expected:" in ASCII strings
expected_offsets = [m.start() for m in re.finditer(b"expected:", data)]
print(f"Found {len(expected_offsets)} occurrences of 'expected:'")

def get_string_at(offset):
    start = offset
    while start > 0 and data[start - 1] != 0:
        start -= 1
    end = offset
    while end < len(data) and data[end] != 0:
        end += 1
    return data[start:end].decode("utf-8", errors="ignore")

for off in expected_offsets:
    s = get_string_at(off)
    if "il.cc" in s or "%s" in s:
        print(f"Offset {hex(off)}: {s}")
