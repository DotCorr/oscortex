import os
from collections import Counter

def inspect():
    ppm_path = "/tmp/qemu-screendump.ppm"
    if not os.path.exists(ppm_path):
        print("PPM not found")
        return
        
    with open(ppm_path, "rb") as f:
        header = f.readline()
        if header != b"P6\n":
            line = f.readline()
            while line.startswith(b"#"):
                line = f.readline()
            size_line = line
        else:
            size_line = f.readline()
            while size_line.startswith(b"#"):
                size_line = f.readline()
                
        width, height = map(int, size_line.split())
        maxval = int(f.readline())
        pixels = f.read()
        
    print(f"Screendump size: {width}x{height}")
    
    # Let's count all colors
    colors = []
    for i in range(0, len(pixels), 3):
        if i + 2 < len(pixels):
            r = pixels[i]
            g = pixels[i+1]
            b = pixels[i+2]
            colors.append((r, g, b))
            
    counter = Counter(colors)
    print("Top 10 colors:")
    for color, count in counter.most_common(10):
        print(f"Color: {color}, Count: {count} ({count/(width*height)*100:.2f}%)")

if __name__ == "__main__":
    inspect()
