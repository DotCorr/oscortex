import urllib.request
import re

url = "https://lightfield.app/_next/static/chunks/fbd3b7beef2d94e1.css"
req = urllib.request.Request(url, headers={'User-Agent': 'Mozilla/5.0'})
with urllib.request.urlopen(req) as response:
    css_data = response.read().decode('utf-8')

print("Searching for --color-neutral-z definitions...")
pattern = r"--color-neutral-z[0-9]+:[^;\n\}]+"
for match in re.finditer(pattern, css_data):
    print(match.group(0))

print("\nSearching for any --color-background-primary-obsidian definition...")
pattern = r"--color-background-primary-obsidian:[^;\n\}]+"
for match in re.finditer(pattern, css_data):
    print(match.group(0))
