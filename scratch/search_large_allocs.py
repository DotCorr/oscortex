log_path = "/Users/ghostportal/.gemini/antigravity/brain/0bfac7bf-ff2a-42ca-ba51-3dd2231cc731/.system_generated/tasks/task-940.log"

lines = []
with open(log_path, "r") as f:
    lines = f.readlines()

i = 0
while i < len(lines):
    line = lines[i].strip()
    if "[sys_mmap]" in line:
        print(line)
        i += 1
    elif "[DL] bump_anon_va: pml4_phys=" in line:
        # Check size
        if "size=0x1000 " not in line:
            print(line)
            # Print the next line if it is existing slot or new slot
            if i + 1 < len(lines) and "[DL] bump_anon_va:" in lines[i+1]:
                print(lines[i+1].strip())
        i += 2
    else:
        i += 1
