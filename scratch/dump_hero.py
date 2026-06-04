import re

file_path = "/Users/ghostportal/.gemini/antigravity/brain/39e73f56-b3cb-4aba-9f54-27fc723c98af/.system_generated/steps/1338/content.md"

with open(file_path, "r", encoding="utf-8") as f:
    content = f.read()

html_content = content.split("---", 1)[1].strip()

# Let's find the section inside main
# We can find <section class='container pt-45 max-[800px]:pt-30'> and grab everything inside it.
start_idx = html_content.find("<section class=\"container pt-45")
if start_idx == -1:
    start_idx = html_content.find("<section class='container pt-45")

if start_idx != -1:
    # Print the next 4000 characters
    print(html_content[start_idx:start_idx+6000])
else:
    print("Could not find section tag. Let's find first section tag:")
    sec_idx = html_content.find("<section")
    print(html_content[sec_idx:sec_idx+4000])
