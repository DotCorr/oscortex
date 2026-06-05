import re

file_path = "/Users/ghostportal/.gemini/antigravity/brain/39e73f56-b3cb-4aba-9f54-27fc723c98af/.system_generated/steps/1338/content.md"

with open(file_path, "r", encoding="utf-8") as f:
    content = f.read()

html_content = content.split("---", 1)[1].strip()

# Let's find all <section tags
sections = []
for m in re.finditer(r"<section[^>]*>", html_content):
    idx = m.start()
    sections.append((m.group(0), idx))

print(f"Found {len(sections)} sections:")
for i, (tag, idx) in enumerate(sections):
    # Print the tag and the first 250 characters of the inner content
    print(f"\nSection {i+1}: {tag}")
    print(html_content[idx:idx+1500])
    print("-" * 60)
