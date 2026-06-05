import urllib.request
import re

url = "https://lightfield.app/_next/static/chunks/fbd3b7beef2d94e1.css"
req = urllib.request.Request(url, headers={'User-Agent': 'Mozilla/5.0'})
with urllib.request.urlopen(req) as response:
    css_data = response.read().decode('utf-8')

# Search for color definitions
print("Searching for specific color codes...")
color_names = [
    'color-background-primary-obsidian',
    'color-neutral-t0', 'color-neutral-t1', 'color-neutral-t2', 'color-neutral-t3', 
    'color-neutral-t4', 'color-neutral-t5', 'color-neutral-t6', 'color-neutral-t7', 
    'color-neutral-t8', 'color-neutral-t9', 'color-neutral-t10', 'color-neutral-t11',
    'color-blue-z5', 'color-blue-z8', 'color-white', 'color-black'
]

for name in color_names:
    pattern = rf"--{name}:[^;\n}}]+"
    for match in re.finditer(pattern, css_data):
        print(match.group(0))
