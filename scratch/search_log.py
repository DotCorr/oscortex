log_path = "/Users/ghostportal/.gemini/antigravity/brain/0bfac7bf-ff2a-42ca-ba51-3dd2231cc731/.system_generated/tasks/task-940.log"

with open(log_path, "r") as f:
    for line in f:
        if "[DL]" in line and "dlopen" in line:
            print(line.strip())
