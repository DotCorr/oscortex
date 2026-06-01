from PIL import Image, ImageDraw

def main():
    # Create a transparent 500x500 canvas
    img = Image.new("RGBA", (500, 500), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)

    # Coordinates from the SVG path
    coords = [
        (155, 60), (225, 120), (250, 100), (275, 120), (345, 60), (325, 130),
        (370, 160), (390, 220), (385, 290), (355, 340), (310, 370), (250, 380),
        (190, 370), (145, 340), (115, 290), (110, 220), (130, 160), (175, 130)
    ]

    # Draw the logo mark filled with violet-400 (#a78bfa / rgb(167, 139, 250))
    draw.polygon(coords, fill=(167, 139, 250, 255))

    # Resize and save as ICO with multiple standard sizes
    # We use Image.Resampling.LANCZOS for high-quality downscaling
    sizes = [(16, 16), (32, 32), (48, 48), (64, 64)]
    images = [img.resize(size, Image.Resampling.LANCZOS) for size in sizes]

    # Save to public/favicon.ico
    public_path = "/Users/ghostportal/Desktop/Dotcorr/OSCortex/landing/public/favicon.ico"
    images[0].save(public_path, format="ICO", sizes=sizes, append_images=images[1:])
    print(f"Successfully generated favicon.ico at: {public_path}")

    # Also save to app/favicon.ico for Next.js App Router default resolution
    app_path = "/Users/ghostportal/Desktop/Dotcorr/OSCortex/landing/app/favicon.ico"
    images[0].save(app_path, format="ICO", sizes=sizes, append_images=images[1:])
    print(f"Successfully generated favicon.ico at: {app_path}")

if __name__ == "__main__":
    main()
