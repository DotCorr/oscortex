import re

log_path = "/Users/ghostportal/.gemini/antigravity/brain/479b9a26-1fa6-4678-9573-e943f2cd0cbf/.system_generated/tasks/task-4412.log"

with open(log_path, 'r') as f:
    lines = f.read().splitlines()

# Let's extract all hex values that represent our trace markers.
# The trace prints lines of 16-hex characters.
# We can find all lines that are exactly 16 hex chars (or 16 uppercase hex chars).
# Wait, some lines might be kernel logs, we can filter those out.
# A trace line has exactly 16 hex characters. Let's write a regex.
hex_pat = re.compile(r'^[0-9A-Fa-f]{16}$')

trace_values = []
for line in lines:
    line_str = line.strip()
    if hex_pat.match(line_str):
        trace_values.append(line_str)

# Now parse the sequence of trace values.
# Each cluster run should print:
# CCCCCCCCCCCCCCCC
# <index>
# <ptr>
# <before>
# <after>
# DDDDDDDDDDDDDDDD
# Note: CodeDeserializationCluster (index 2 or index 6) prints BBBBBBBBBBBBBBBB and EEEEEEEEEEEEEEEE.
# We can treat those specially.

i = 0
n = len(trace_values)
traces = []

while i < n:
    val = trace_values[i]
    if val == "CCCCCCCCCCCCCCCC":
        if i + 3 < n:
            idx = int(trace_values[i+1], 16)
            ptr = int(trace_values[i+2], 16)
            before = int(trace_values[i+3], 16)
            
            # Check if this is Code cluster which does custom tracing
            # Wait, Code cluster has after state or DDDD?
            # Let's look for the next DDDDDDDDDDDDDDDD or EEEEEEEEEEEEEEEE
            after = None
            j = i + 4
            is_code = False
            while j < min(i + 30, n):
                if trace_values[j] == "BBBBBBBBBBBBBBBB":
                    is_code = True
                if trace_values[j] == "DDDDDDDDDDDDDDDD" or trace_values[j] == "EEEEEEEEEEEEEEEE":
                    after = int(trace_values[j-1], 16)
                    break
                j += 1
            traces.append((idx, ptr, before, after, is_code))
            i = j
    i += 1

print(f"Index | Pointer | Before | After | Delta | Type")
print("-" * 70)
for idx, ptr, before, after, is_code in traces:
    type_str = "Code" if is_code else "Normal"
    if after is not None:
        delta = after - before
        print(f"{idx:5d} | {ptr:016x} | {before:09x} | {after:09x} | {delta:10d} | {type_str}")
    else:
        print(f"{idx:5d} | {ptr:016x} | {before:09x} | CRASHED    |            | {type_str}")
