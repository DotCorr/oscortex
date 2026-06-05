import urllib.request
import re

css_urls = [
    "https://lightfield.app/_next/static/chunks/aea53d880a30d667.css",
    "https://lightfield.app/_next/static/chunks/fbd3b7beef2d94e1.css",
    "https://lightfield.app/_next/static/chunks/aa1147ab3fc56c52.css"
]

for url in css_urls:
    try:
        print(f"Fetching: {url}")
        req = urllib.request.Request(url, headers={'User-Agent': 'Mozilla/5.0'})
        with urllib.request.urlopen(req) as response:
            css_data = response.read().decode('utf-8')
        
        # Let's search for interesting patterns in the CSS
        # Search for text-h1, text-d4, pt-45, monocaps-xxs, variables in :root
        print("Length:", len(css_data))
        
        root_matches = re.findall(r":root\s*\{[^}]*\}", css_data)
        if root_matches:
            print("Found :root variables:")
            for match in root_matches:
                print(match)
                
        # Look for text-h1
        h1_matches = re.findall(r"\.text-h1\s*\{[^}]*\}", css_data)
        if h1_matches:
            print("Found .text-h1:")
            for match in h1_matches:
                print(match)
                
        # Look for pt-45
        pt45_matches = re.findall(r"\.pt-45\s*\{[^}]*\}", css_data)
        if pt45_matches:
            print("Found .pt-45:")
            for match in pt45_matches:
                print(match)

        # Look for font-size definitions
        font_matches = re.findall(r"\.text-[a-z0-9-]+\s*\{[^}]*font-size[^}]*\}", css_data)
        if font_matches:
            print("Found some text styles (first 10):")
            for match in font_matches[:10]:
                print(match)
                
    except Exception as e:
        print(f"Error fetching {url}: {e}")
