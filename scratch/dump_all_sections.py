import re

file_path = "/Users/ghostportal/.gemini/antigravity/brain/39e73f56-b3cb-4aba-9f54-27fc723c98af/.system_generated/steps/1338/content.md"

with open(file_path, "r", encoding="utf-8") as f:
    content = f.read()

html_content = content.split("---", 1)[1].strip()

# Let's save a clean HTML structure file in scratch directory
out_path = "/Users/ghostportal/Desktop/Dotcorr/OSCortex/scratch/lightfield_structure.html"

with open(out_path, "w", encoding="utf-8") as f:
    f.write(html_content)

print(f"Dumped complete Lightfield HTML to {out_path}")
