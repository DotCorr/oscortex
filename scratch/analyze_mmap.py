import re

def main():
    log_path = "/Users/ghostportal/.gemini/antigravity/brain/479b9a26-1fa6-4678-9573-e943f2cd0cbf/.system_generated/tasks/task-912.log"
    with open(log_path, "r") as f:
        for line in f:
            if "Segment" in line or "loaded" in line or "map_library" in line or "dlopen" in line:
                if "bump_anon_va" not in line:
                    print(line.strip())

if __name__ == "__main__":
    main()
