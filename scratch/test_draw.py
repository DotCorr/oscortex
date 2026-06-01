from PIL import Image, ImageDraw

def test_draw():
    # Create a 500x500 black canvas (like Image 1)
    img = Image.new("RGBA", (500, 500), (0, 0, 0, 255))
    draw = ImageDraw.Draw(img)

    # 1. Head polygon vertices (clockwise)
    head_coords = [
        (175, 130), (130, 160), (110, 220), (115, 290), (145, 340), (190, 370),
        (250, 380), (310, 370), (355, 340), (385, 290), (390, 220), (370, 160),
        (325, 130), (275, 120), (250, 100), (225, 120)
    ]
    # Fill head with white
    draw.polygon(head_coords, fill=(255, 255, 255, 255))

    # 2. Draw ear outline lines (white, thickness 14)
    # Left ear: 175,130 -> 155,60 -> 225,120
    draw.line([(175, 130), (155, 60), (225, 120)], fill=(255, 255, 255, 255), width=14, joint="miter")
    # Right ear: 275,120 -> 345,60 -> 325,130
    draw.line([(275, 120), (345, 60), (325, 130)], fill=(255, 255, 255, 255), width=14, joint="miter")

    # 3. Draw cutouts (black, matching background)
    # Left eye: circle at 186, 226, radius 22
    draw.ellipse([186 - 22, 226 - 22, 186 + 22, 226 + 22], fill=(0, 0, 0, 255))
    # Right eye: circle at 314, 226, radius 22
    draw.ellipse([314 - 22, 226 - 22, 314 + 22, 226 + 22], fill=(0, 0, 0, 255))
    # Nose: ellipse at 250, 280, rx 25, ry 15
    draw.ellipse([250 - 25, 280 - 15, 250 + 25, 280 + 15], fill=(0, 0, 0, 255))
    # Mouth line: vertical rectangle from x=246 to 254, y=280 to 340
    draw.rectangle([246, 280, 254, 340], fill=(0, 0, 0, 255))

    # Save to scratch/test_logo.png
    output_path = "/Users/ghostportal/Desktop/Dotcorr/OSCortex/scratch/test_logo.png"
    img.save(output_path)
    print(f"Test rendering saved to: {output_path}")

if __name__ == "__main__":
    test_draw()
