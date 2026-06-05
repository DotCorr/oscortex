from PIL import Image, ImageDraw

def main():
    # 1. Create a grayscale mask (L mode) of size 500x500, filled with 0 (black / transparent)
    mask = Image.new("L", (500, 500), 0)
    draw_mask = ImageDraw.Draw(mask)

    # 2. Draw head polygon on mask with 255 (opaque white)
    head_coords = [
        (175, 130), (130, 160), (110, 220), (115, 290), (145, 340), (190, 370),
        (250, 380), (310, 370), (355, 340), (385, 290), (390, 220), (370, 160),
        (325, 130), (275, 120), (250, 100), (225, 120)
    ]
    draw_mask.polygon(head_coords, fill=255)

    # 3. Draw ear outlines on mask with 255 (opaque white) and width 14
    draw_mask.line([(175, 130), (155, 60), (225, 120)], fill=255, width=14, joint="miter")
    draw_mask.line([(275, 120), (345, 60), (325, 130)], fill=255, width=14, joint="miter")

    # 4. Draw eyes, nose, and mouth cutouts on mask with 0 (transparent black)
    # Left eye: circle at 186, 226, radius 22
    draw_mask.ellipse([186 - 22, 226 - 22, 186 + 22, 226 + 22], fill=0)
    # Right eye: circle at 314, 226, radius 22
    draw_mask.ellipse([314 - 22, 226 - 22, 314 + 22, 226 + 22], fill=0)
    # Nose: ellipse at 250, 280, rx 25, ry 15
    draw_mask.ellipse([250 - 25, 280 - 15, 250 + 25, 280 + 15], fill=0)
    # Mouth line: vertical rectangle from x=246 to 254, y=280 to 340
    draw_mask.rectangle([246, 280, 254, 340], fill=0)

    # 5. Create a solid color image (RGBA) filled with the brand color violet-400: rgb(167, 139, 250)
    violet_img = Image.new("RGBA", (500, 500), (167, 139, 250, 255))
    violet_img.putalpha(mask)

    # Resize and save as ICO with multiple standard sizes (16x16, 32x32, 48x48, 64x64)
    sizes = [(16, 16), (32, 32), (48, 48), (64, 64)]
    images = [violet_img.resize(size, Image.Resampling.LANCZOS) for size in sizes]

    # Save to public/favicon.ico
    public_path = "/Users/ghostportal/Desktop/Dotcorr/OSCortex/landing/public/favicon.ico"
    images[0].save(public_path, format="ICO", sizes=sizes, append_images=images[1:])
    print(f"Generated detailed violet favicon at: {public_path}")

    # Save to app/favicon.ico
    app_path = "/Users/ghostportal/Desktop/Dotcorr/OSCortex/landing/app/favicon.ico"
    images[0].save(app_path, format="ICO", sizes=sizes, append_images=images[1:])
    print(f"Generated detailed violet favicon at: {app_path}")

if __name__ == "__main__":
    main()
