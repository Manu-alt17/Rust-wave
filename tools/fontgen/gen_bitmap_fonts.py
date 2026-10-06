"""Generate the firmware's 1-bpp bitmap font strikes.

Writes three Rust files:

  src/app/typography/assets.rs          UI text: Inter Medium / SemiBold
  src/app/reader_literata_assets.rs     book text: Literata Medium
  src/app/reader_atkinson_next_assets.rs  book text: Atkinson Hyperlegible Next Medium

Every strike carries printable ASCII in a table indexed by code point, plus
a sorted table of extra glyphs: all of Latin-1 and the typographic
characters of Windows-1252 (curly quotes, dashes, ellipsis, euro...). Books
in Italian, French, Spanish or German use them on nearly every line; before,
each one was folded to ASCII or shown as '?'.

The sizes, weights, optical sizes and thresholds reproduce the strikes this
firmware shipped with (measured against them, see RASTER NOTES below), so
screens keep their layout and only gain the new characters.

Usage (Pillow required; the TTFs are the variable fonts from
github.com/google/fonts, SIL Open Font License 1.1):

    python tools/fontgen/gen_bitmap_fonts.py --fonts DIR [--preview out.png]

DIR must contain Inter[opsz,wght].ttf, Literata[opsz,wght].ttf and
AtkinsonHyperlegibleNext[wght].ttf. Run rustfmt on the outputs afterwards
(cargo fmt does it).

RASTER NOTES
- Glyphs are drawn one at a time at an integer origin with FreeType's
  hinting (Pillow's default) and anti-aliasing, then thresholded: 96 for
  book text, 128 for the UI. The lower threshold keeps book strokes solid.
- Book advances are FreeType's hinted advances (Pillow basic layout). UI
  advances round the unhinted advance up, which spaces the small UI sizes a
  little more openly.
- Each glyph is stored as its tight ink box (left/top relative to the pen
  position on the baseline), so blank margins cost no flash.
"""
import argparse
import math
import os
import sys

from PIL import Image, ImageDraw, ImageFont

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))

ASCII = [chr(code) for code in range(0x20, 0x7F)]
# Latin-1 (U+00A0..U+00FF, soft hyphen included: it is an empty glyph with
# no advance) plus the characters Windows-1252 puts in 0x80..0x9F.
EXTRA = sorted(
    [chr(code) for code in range(0xA0, 0x100)]
    + [
        "€",  # euro
        "‚",  # single low-9 quote
        "ƒ",  # f with hook
        "„",  # double low-9 quote
        "…",  # ellipsis
        "†",  # dagger
        "‡",  # double dagger
        "ˆ",  # modifier circumflex
        "‰",  # per mille
        "Š",  # S caron
        "‹",  # single left angle quote
        "Œ",  # OE
        "Ž",  # Z caron
        "‘",  # left single quote
        "’",  # right single quote / apostrophe
        "“",  # left double quote
        "”",  # right double quote
        "•",  # bullet
        "–",  # en dash
        "—",  # em dash
        "˜",  # small tilde
        "™",  # trade mark
        "š",  # s caron
        "›",  # single right angle quote
        "œ",  # oe
        "ž",  # z caron
        "Ÿ",  # Y diaeresis
    ]
)
# Characters that advance but draw nothing.
BLANK = {" ": " ", "­": None}
# Drawn with another code point's glyph when the font lacks their own
# (Atkinson Hyperlegible Next has the Greek mu but no micro sign).
SUBSTITUTES = {"µ": "μ"}

INTER = "Inter[opsz,wght].ttf"
LITERATA = "Literata[opsz,wght].ttf"
ATKINSON = "AtkinsonHyperlegibleNext[wght].ttf"

MEDIUM = 500
SEMIBOLD = 600

# (name, font file, pixel size, variation axes, line height)
UI_STRIKES = [
    ("INTER_COMPACT_BODY", 13, MEDIUM, 18),
    ("INTER_COMPACT_HEADING", 18, SEMIBOLD, 24),
    ("INTER_COMPACT_LARGE", 22, SEMIBOLD, 29),
    ("INTER_STANDARD_BODY", 15, MEDIUM, 20),
    ("INTER_STANDARD_HEADING", 20, SEMIBOLD, 26),
    ("INTER_STANDARD_LARGE", 24, SEMIBOLD, 31),
    ("INTER_LARGE_BODY", 17, MEDIUM, 23),
    ("INTER_LARGE_HEADING", 22, SEMIBOLD, 29),
    ("INTER_LARGE_LARGE", 26, SEMIBOLD, 34),
]
# Literata's optical size differs per strike on purpose: the two smaller
# strikes shipped with the display-leaning cut (opsz = size - 1), the two
# larger ones with the default text cut (opsz 12).
LITERATA_STRIKES = [
    ("LITERATA_LARGE", 23, 22, 28),
    ("LITERATA_XLARGE", 27, 26, 33),
    ("LITERATA_XXLARGE", 31, 12, 38),
    ("LITERATA_XXXLARGE", 35, 12, 44),
]
ATKINSON_STRIKES = [
    ("ATKINSON_NEXT_LARGE", 23, 28),
    ("ATKINSON_NEXT_XLARGE", 27, 33),
    ("ATKINSON_NEXT_XXLARGE", 31, 38),
    ("ATKINSON_NEXT_XXXLARGE", 35, 44),
]


# Glyphs that FreeType's hinting breaks at one size, redrawn by hand: the ink
# box's left and top relative to the pen position on the baseline, then its
# rows ('#' is ink). Keyed by strike name and character. The advance stays
# the font's own.
# None at present: the one there was (the percent sign of Inter Medium 12 px,
# whose rings and slash the hinting collapsed) went with the 11 to 13 px
# "detail" strikes, which were too small to read on the panel and are no
# longer generated.
GLYPH_OVERRIDES = {}


class Strike:
    def __init__(self, name, path, size, axes, line_height, threshold, hinted_advance, label):
        self.name = name
        self.line_height = line_height
        self.threshold = threshold
        self.label = label
        self.size = size
        self.basic = ImageFont.truetype(path, size, layout_engine=ImageFont.Layout.BASIC)
        self.basic.set_variation_by_axes(axes)
        self.shaped = ImageFont.truetype(path, size, layout_engine=ImageFont.Layout.RAQM)
        self.shaped.set_variation_by_axes(axes)
        self.hinted_advance = hinted_advance
        self.bitmap = bytearray()
        self.glyphs = {}

    def advance(self, ch):
        if self.hinted_advance:
            return int(round(self.basic.getlength(ch)))
        return int(math.ceil(self.shaped.getlength(ch) - 1e-3))

    def ink(self, ch):
        """Tight ink box of one glyph drawn at the pen origin on the baseline."""
        canvas = self.size * 4
        origin_x, baseline = self.size, self.size * 3
        image = Image.new("L", (canvas, canvas), 0)
        draw = ImageDraw.Draw(image)
        draw.fontmode = "L"
        draw.text((origin_x, baseline), ch, font=self.basic, fill=255, anchor="ls")
        mask = image.point(lambda value: 255 if value >= self.threshold else 0)
        box = mask.getbbox()
        if box is None:
            return None
        left, top, right, bottom = box
        rows = []
        pixels = mask.load()
        for y in range(top, bottom):
            rows.append([pixels[x, y] != 0 for x in range(left, right)])
        return left - origin_x, top - baseline, right - left, bottom - top, rows

    def add(self, ch):
        if ch in BLANK:
            source = BLANK[ch]
            advance = self.advance(source) if source else 0
            self.glyphs[ch] = (0, 0, 0, advance, 0, 0)
            return
        drawn = ch
        if not has_glyph(self.basic, drawn):
            drawn = SUBSTITUTES.get(ch)
            if drawn is None or not has_glyph(self.basic, drawn):
                if ch in ASCII:
                    sys.exit(f"{self.name}: font has no glyph for U+{ord(ch):04X}")
                # Left out of the table: the firmware shows '?' for it.
                print(f"  {self.name}: no glyph for U+{ord(ch):04X}, skipped")
                return
        advance = self.advance(drawn)
        override = GLYPH_OVERRIDES.get((self.name, ch))
        if override is not None:
            left, top, art = override
            rows = [[cell == "#" for cell in row] for row in art]
            ink = (left, top, len(art[0]), len(art), rows)
        else:
            ink = self.ink(drawn)
        if ink is None:
            self.glyphs[ch] = (0, 0, 0, advance, 0, 0)
            return
        left, top, width, height, rows = ink
        offset = len(self.bitmap)
        for row in rows:
            for start in range(0, width, 8):
                byte = 0
                for bit, on in enumerate(row[start:start + 8]):
                    if on:
                        byte |= 0x80 >> bit
                self.bitmap.append(byte)
        for value, low, high in [(width, 0, 255), (height, 0, 255), (advance, 0, 255), (left, -128, 127), (top, -128, 127)]:
            if not low <= value <= high:
                sys.exit(f"{self.name}: U+{ord(ch):04X} metric {value} out of range")
        self.glyphs[ch] = (offset, width, height, advance, left, top)

    def build(self):
        for ch in ASCII + EXTRA:
            self.add(ch)
        return self


def rendered(font, ch):
    size = int(font.size)
    image = Image.new("L", (size * 3, size * 3), 0)
    ImageDraw.Draw(image).text((size, size * 2), ch, font=font, fill=255, anchor="ls")
    return image.tobytes()


def has_glyph(font, ch):
    # A missing glyph renders as .notdef: compare against a private-use code
    # point no Latin font covers.
    return ch.isspace() or rendered(font, ch) != rendered(font, "\U000F0000")


def rust_glyph(glyph):
    return "g({}, {}, {}, {}, {}, {})".format(*glyph)


def emit(strikes, header, use_line, path):
    out = [header, "", use_line, ""]
    for strike in strikes:
        out.append(f"// {strike.name}: {strike.label}, line-height {strike.line_height}")
        out.append(f"const {strike.name}_GLYPHS: [Glyph; 95] = [")
        for ch in ASCII:
            out.append(f"    {rust_glyph(strike.glyphs[ch])},")
        out.append("];")
        extra = [ch for ch in EXTRA if ch in strike.glyphs]
        out.append(f"const {strike.name}_EXTRA: [(char, Glyph); {len(extra)}] = [")
        for ch in extra:
            out.append(f"    ('\\u{{{ord(ch):x}}}', {rust_glyph(strike.glyphs[ch])}),")
        out.append("];")
        out.append(f"const {strike.name}_BITMAP: [u8; {len(strike.bitmap)}] = [")
        for start in range(0, len(strike.bitmap), 16):
            chunk = strike.bitmap[start:start + 16]
            out.append("    " + " ".join(f"0x{b:02X}," for b in chunk))
        out.append("];")
        out.append(f"pub static {strike.name}: BitmapFont = BitmapFont {{")
        out.append(f"    glyphs: &{strike.name}_GLYPHS,")
        out.append(f"    bitmap: &{strike.name}_BITMAP,")
        out.append(f"    extra: &{strike.name}_EXTRA,")
        out.append(f"    line_height: {strike.line_height},")
        out.append("};")
        out.append("")
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write("\n".join(out))
    total = sum(len(s.bitmap) for s in strikes)
    print(f"wrote {os.path.relpath(path, REPO)}: {len(strikes)} strikes, {total} bitmap bytes")


def draw_text(image, x, baseline, strike, text):
    pixels = image.load()
    for ch in text:
        glyph = strike.glyphs.get(ch) or strike.glyphs["?"]
        offset, width, height, advance, left, top = glyph
        stride = (width + 7) // 8
        for row in range(height):
            for column in range(width):
                byte = strike.bitmap[offset + row * stride + column // 8]
                if byte & (0x80 >> (column % 8)):
                    px, py = x + left + column, baseline + top + row
                    if 0 <= px < image.width and 0 <= py < image.height:
                        pixels[px, py] = 0
        x += advance
    return x


def preview(path, groups):
    """Rebuild sample lines from the generated bytes, as the firmware draws them."""
    sample = [
        "Perché l’uomo è «piccolo»? Città, virtù, poiché — “sì”…",
        "À È É Ì Ò Ù ç ñ ü ß œ € 12,50 – Naïve façade",
    ]
    lines = []
    for strikes in groups:
        for strike in strikes:
            lines.append((strike, sample))
    height = 20 + sum((s.line_height + 4) * len(t) + 8 for s, t in lines)
    image = Image.new("1", (1100, height), 1)
    y = 10
    for strike, texts in lines:
        for text in texts:
            y += strike.line_height
            draw_text(image, 10, y, strike, text)
            y += 4
        y += 8
    image.save(path)
    print(f"wrote preview {path}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--fonts", required=True, help="directory with the source TTFs")
    parser.add_argument("--preview", help="optional PNG with sample lines")
    args = parser.parse_args()

    def font_path(name):
        path = os.path.join(args.fonts, name)
        if not os.path.exists(path):
            sys.exit(f"missing {path}")
        return path

    ui = [
        Strike(name, font_path(INTER), size, [14, weight], line_height, 128, False,
               f"Inter {'Medium' if weight == MEDIUM else 'SemiBold'} {size} px")
        .build()
        for name, size, weight, line_height in UI_STRIKES
    ]
    literata = [
        Strike(name, font_path(LITERATA), size, [opsz, MEDIUM], line_height, 96, True,
               f"Literata Medium {size} px, opsz {opsz}")
        .build()
        for name, size, opsz, line_height in LITERATA_STRIKES
    ]
    atkinson = [
        Strike(name, font_path(ATKINSON), size, [MEDIUM], line_height, 96, True,
               f"Atkinson Hyperlegible Next Medium {size} px")
        .build()
        for name, size, line_height in ATKINSON_STRIKES
    ]

    notice = (
        "//! Generated by tools/fontgen/gen_bitmap_fonts.py; do not edit by hand.\n"
        "//!\n"
        "//! {what}\n"
        "//! Printable ASCII plus Latin-1 and the Windows-1252 typographic\n"
        "//! characters. Rasterized from {source} (SIL Open Font License 1.1,\n"
        "//! see docs/licenses/FONT_NOTICES.md); the font files themselves are\n"
        "//! not distributed."
    )
    emit(
        ui,
        notice.format(what="UI text strikes.", source="Inter"),
        "use super::{g, BitmapFont, Glyph};",
        os.path.join(REPO, "src", "app", "typography", "assets.rs"),
    )
    emit(
        literata,
        notice.format(what="Reader body strikes.", source="Literata"),
        "use super::typography::{g, BitmapFont, Glyph};",
        os.path.join(REPO, "src", "app", "reader_literata_assets.rs"),
    )
    emit(
        atkinson,
        notice.format(what="Reader body strikes.", source="Atkinson Hyperlegible Next"),
        "use super::typography::{g, BitmapFont, Glyph};",
        os.path.join(REPO, "src", "app", "reader_atkinson_next_assets.rs"),
    )
    if args.preview:
        preview(args.preview, [ui, literata, atkinson])


if __name__ == "__main__":
    main()
