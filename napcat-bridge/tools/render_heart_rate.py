#!/usr/bin/env python3
"""Overlay an exact three-digit heart-rate value on the generated background."""
import argparse
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont


FONT_CANDIDATES = (
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf",
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSansCondensed-Bold.ttf",
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf",
)


def load_font(size: int):
    for candidate in FONT_CANDIDATES:
        path = Path(candidate)
        if path.is_file():
            return ImageFont.truetype(path, size=size)
    return ImageFont.load_default()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("bpm", type=int, help="heart rate from 0 to 999")
    parser.add_argument("--background", type=Path, default=Path("assets/heart-rate/background.png"))
    parser.add_argument("--output", type=Path)
    parser.add_argument("--size", type=int, default=640, help="output width and height (default: 640)")
    parser.add_argument("--quality", type=int, default=95, help="JPEG quality from 1 to 100 (default: 95)")
    args = parser.parse_args()
    if not 0 <= args.bpm <= 999:
        parser.error("bpm must be between 0 and 999")
    if args.size < 64:
        parser.error("size must be at least 64 pixels")
    if not 1 <= args.quality <= 100:
        parser.error("quality must be between 1 and 100")
    if not args.background.exists():
        parser.error(f"background not found: {args.background}")

    image = Image.open(args.background).convert("RGBA")
    if image.size != (args.size, args.size):
        image = image.resize((args.size, args.size), Image.Resampling.LANCZOS)
    draw = ImageDraw.Draw(image)
    number = f"{args.bpm:03d}"
    font = load_font(size=int(image.width * 0.22))
    box = draw.textbbox((0, 0), number, font=font, stroke_width=3)
    x = (image.width - (box[2] - box[0])) // 2
    y = (image.height - (box[3] - box[1])) // 2 - box[1]
    draw.text(
        (x, y),
        number,
        font=font,
        fill=(255, 255, 255, 255),
        stroke_width=max(3, image.width // 300),
        stroke_fill=(5, 12, 30, 230),
    )

    output = args.output or args.background.with_name(f"heart-rate-{number}.png")
    output.parent.mkdir(parents=True, exist_ok=True)
    rgb_image = image.convert("RGB")
    if output.suffix.lower() in {".jpg", ".jpeg"}:
        rgb_image.save(output, format="JPEG", quality=args.quality, subsampling=0, optimize=True, progressive=True)
    else:
        rgb_image.save(output, format="PNG", optimize=True)
    print(output)


if __name__ == "__main__":
    main()
