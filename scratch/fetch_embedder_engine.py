import urllib.request

files = [
    "shell.h",
    "engine.h",
    "platform_view.h",
    "rasterizer.h",
    "animator.h",
    "vsync_waiter.h"
]

for filename in files:
    url = f"https://raw.githubusercontent.com/flutter/engine/main/shell/common/{filename}"
    req = urllib.request.Request(url, headers={'User-Agent': 'Mozilla/5.0'})
    try:
        with urllib.request.urlopen(req) as response:
            content = response.read().decode('utf-8')
            lines = content.split('\n')
            for i, line in enumerate(lines):
                if "std::map" in line or "std::unordered_map" in line:
                    print(f"{filename} line {i+1}: {line}")
    except Exception as e:
        pass
