from html.parser import HTMLParser
import json

file_path = "/Users/ghostportal/.gemini/antigravity/brain/39e73f56-b3cb-4aba-9f54-27fc723c98af/.system_generated/steps/1338/content.md"

with open(file_path, "r", encoding="utf-8") as f:
    content = f.read()

html_content = content.split("---", 1)[1].strip()

class StructureParser(HTMLParser):
    def __init__(self):
        super().__init__()
        self.depth = 0
        self.interesting_tags = {'header', 'nav', 'main', 'section', 'div', 'h1', 'h2', 'p', 'form', 'input', 'button'}

    def handle_starttag(self, tag, attrs):
        if tag in self.interesting_tags:
            attrs_dict = dict(attrs)
            cls = attrs_dict.get('class', '')
            # Only print key elements or small nesting
            if self.depth < 6:
                print("  " * self.depth + f"<{tag} class='{cls}'>")
            self.depth += 1

    def handle_endtag(self, tag):
        if tag in self.interesting_tags:
            self.depth -= 1
            if self.depth < 5:
                print("  " * self.depth + f"</{tag}>")

parser = StructureParser()
parser.feed(html_content)
