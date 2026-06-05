import urllib.request
import re

url = "https://lightfield.app/_next/static/chunks/fbd3b7beef2d94e1.css"
req = urllib.request.Request(url, headers={'User-Agent': 'Mozilla/5.0'})
with urllib.request.urlopen(req) as response:
    css_data = response.read().decode('utf-8')

print("Searching for dark mode CSS blocks...")
# Find blocks that have .dark or prefers-color-scheme: dark
# Let's print sections that contain variables starting with --color-neutral-t
# Usually it's like: .dark { ... }
dark_blocks = []
for match in re.finditer(r"\.dark\s*\{[^}]*\}", css_data):
    dark_blocks.append(match.group(0))

for match in re.finditer(r"@media\s*\(\s*prefers-color-scheme\s*:\s*dark\s*\)\s*\{[^}]*\}", css_data):
    dark_blocks.append(match.group(0))

print(f"Found {len(dark_blocks)} dark mode blocks:")
for block in dark_blocks:
    # Print variables inside it
    lines = block.split("\n")
    for line in lines:
        if "--color-neutral" in line or "--color-background" in line or "--theme-bg" in line:
            print(line.strip())
