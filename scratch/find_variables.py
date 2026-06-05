import urllib.request
import re

url = "https://lightfield.app/_next/static/chunks/fbd3b7beef2d94e1.css"
req = urllib.request.Request(url, headers={'User-Agent': 'Mozilla/5.0'})
with urllib.request.urlopen(req) as response:
    css_data = response.read().decode('utf-8')

# Search for any variable definitions
# E.g. --text-h1: ... or --text-content-primary: ...
# Let's find matches in root or html
print("Searching for color/text variable definitions...")
variables = {}
for match in re.finditer(r"--[a-zA-Z0-9-]+:[^;\n}]+", css_data):
    var_def = match.group(0)
    name, val = var_def.split(":", 1)
    variables[name.strip()] = val.strip()

# Print variables that contain text-h1, text-h2, d4, font, content, bg, surface, border
interesting = ['text-h', 'd4', 'content', 'bg', 'surface', 'border', 'spacing']
for name, val in sorted(variables.items()):
    if any(k in name for k in interesting):
        print(f"{name}: {val}")
