"""Test JPEGs for src/jpeg_luma.rs, written to testdata/jpeg/.

The same synthetic picture in every flavour the luma-only decoder has to
read: baseline and progressive, 4:2:0, 4:2:2, 4:4:4 and grayscale,
optimized Huffman tables, restart markers, plus a CMYK file it has to hand
back to jpeg-decoder. 173x229 is a multiple of neither 8 nor 16, so every
file has partial blocks and partial MCUs on both edges.

The unit tests compare the decoder with jpeg-decoder on these files, so
only the JPEGs are kept, never any expected output. Needs Pillow:

    python tools/jpegluma/gen_fixtures.py
"""

import pathlib
import random

from PIL import Image, ImageDraw

WIDTH, HEIGHT = 173, 229
OUT = pathlib.Path(__file__).resolve().parents[2] / "testdata" / "jpeg"


def picture():
    """Gradients (low frequencies), hard edges and text (high frequencies),
    saturated colours (chroma the decoder must skip) and noise."""
    image = Image.new("RGB", (WIDTH, HEIGHT))
    pixels = image.load()
    for y in range(HEIGHT):
        for x in range(WIDTH):
            pixels[x, y] = (
                x * 255 // (WIDTH - 1),
                y * 255 // (HEIGHT - 1),
                (x + y) * 255 // (WIDTH + HEIGHT - 2),
            )
    draw = ImageDraw.Draw(image)
    draw.rectangle([20, 30, 90, 80], fill=(250, 250, 250))
    draw.text((25, 40), "Verity 123", fill=(0, 0, 0))
    draw.ellipse([60, 100, 150, 190], fill=(10, 20, 200), outline=(255, 255, 0), width=3)
    for x in range(0, WIDTH, 6):
        colour = (0, 0, 0) if (x // 6) % 2 else (255, 255, 255)
        draw.line([(x, 200), (x, HEIGHT - 1)], fill=colour)
    rng = random.Random(7)
    for _ in range(300):
        pixels[rng.randrange(WIDTH), rng.randrange(HEIGHT)] = (
            rng.randrange(256),
            rng.randrange(256),
            rng.randrange(256),
        )
    return image


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    rgb = picture()
    variants = {
        "baseline_420.jpg": (rgb, dict(quality=85, subsampling=2)),
        "baseline_422_restart.jpg": (rgb, dict(quality=85, subsampling=1, restart_marker_rows=1)),
        "progressive_420.jpg": (rgb, dict(quality=85, subsampling=2, progressive=True)),
        "progressive_422.jpg": (rgb, dict(quality=85, subsampling=1, progressive=True)),
        "progressive_444_q95.jpg": (rgb, dict(quality=95, subsampling=0, progressive=True)),
        "progressive_gray.jpg": (rgb.convert("L"), dict(quality=85, progressive=True)),
        "progressive_optimized_restart.jpg": (
            rgb,
            dict(quality=85, subsampling=2, progressive=True, optimize=True, restart_marker_blocks=5),
        ),
        "cmyk.jpg": (rgb.convert("CMYK"), dict(quality=85)),
    }
    for name, (image, options) in variants.items():
        image.save(OUT / name, "JPEG", **options)
        print(f"{name}: {(OUT / name).stat().st_size} bytes")


if __name__ == "__main__":
    main()
