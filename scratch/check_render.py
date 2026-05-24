import subprocess
import time
import os

def check():
    ppm_path = "/tmp/qemu-screendump.ppm"
    png_path = "/Users/ghostportal/.gemini/antigravity/brain/579d2a6e-f388-46a1-8c11-2e8d8d30b78f/qemu-screendump-current.png"
    
    # 1. Take screendump
    subprocess.run("echo \"screendump " + ppm_path + "\" | nc -U /tmp/qemu-monitor.sock", shell=True)
    
    # 2. Convert to png
    subprocess.run(f"sips -s format png {ppm_path} --out {png_path}", shell=True, stdout=subprocess.DEVNULL)
    
    if not os.path.exists(ppm_path):
        print("Screendump failed: ppm not found")
        return False
        
    # Read PPM header and pixels
    # PPM binary format (P6):
    # P6
    # width height
    # maxval
    # binary RGBRGBRGB...
    with open(ppm_path, "rb") as f:
        header = f.readline()
        if header != b"P6\n":
            # Skip comments
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
    
    # Let's count unique pixel colors in the top half (e.g. y < height - 60)
    # The default surface color for ID 2 (green) is R=82, G=110, B=78
    unique_colors = set()
    total_pixels = 0
    non_green_pixels = 0
    
    # Let's check the top half (0 to height - 100)
    for y in range(0, height - 100):
        for x in range(0, width):
            idx = (y * width + x) * 3
            if idx + 2 < len(pixels):
                r = pixels[idx]
                g = pixels[idx+1]
                b = pixels[idx+2]
                color = (r, g, b)
                unique_colors.add(color)
                # Check if it's NOT the olive green (82, 110, 78) and NOT the background color (16, 24, 32)
                # Allow a small tolerance for both
                is_green = abs(r - 82) < 5 and abs(g - 110) < 5 and abs(b - 78) < 5
                is_bg = abs(r - 16) < 10 and abs(g - 24) < 10 and abs(b - 32) < 10
                if not is_green and not is_bg:
                    non_green_pixels += 1
                total_pixels += 1
                
    print(f"Unique colors in active area: {len(unique_colors)}")
    print(f"Non-green, non-bg pixels in active area: {non_green_pixels} / {total_pixels} ({non_green_pixels/total_pixels*100:.2f}%)")
    
    if non_green_pixels > 100:
        print("SUCCESS: UI has rendered non-green pixels!")
        return True
    else:
        print("STILL GREEN: Only olive green and bg detected in active area.")
        return False

if __name__ == "__main__":
    check()
