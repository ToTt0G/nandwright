"""Draws the extension icon: a NAND gate. Run once; the PNG is committed.

    python3 scripts/make-icon.py
"""

from pathlib import Path

from PIL import Image, ImageDraw

SCALE = 4  # drawn large, then scaled down for smooth edges
SIZE = 256 * SCALE
BACKGROUND = (31, 36, 48, 255)
BODY = (43, 50, 69, 255)
LINE = (255, 204, 102, 255)
WIDTH = 13 * SCALE


def s(value: float) -> int:
    return round(value * SCALE)


image = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
draw = ImageDraw.Draw(image)
draw.rounded_rectangle((0, 0, SIZE - 1, SIZE - 1), radius=s(52), fill=BACKGROUND)

# The gate body: a flat left edge and a semicircular right edge.
left, top, bottom = s(74), s(72), s(184)
middle = (top + bottom) // 2
radius = (bottom - top) // 2
straight = s(118)
draw.rectangle((left, top, straight, bottom), fill=BODY)
draw.pieslice((straight - radius, top, straight + radius, bottom), -90, 90, fill=BODY)
draw.line((left, top, straight, top), fill=LINE, width=WIDTH)
draw.line((left, bottom, straight, bottom), fill=LINE, width=WIDTH)
draw.line((left, top - WIDTH // 2 + 1, left, bottom + WIDTH // 2 - 1), fill=LINE, width=WIDTH)
draw.arc(
    (straight - radius - WIDTH // 2, top - WIDTH // 2, straight + radius + WIDTH // 2, bottom + WIDTH // 2),
    -90,
    90,
    fill=LINE,
    width=WIDTH,
)

# The two inputs.
for y in (s(104), s(152)):
    draw.line((s(36), y, left, y), fill=LINE, width=WIDTH)
    draw.ellipse((s(36) - WIDTH // 2, y - WIDTH // 2, s(36) + WIDTH // 2, y + WIDTH // 2), fill=LINE)

# The inverting bubble and the output.
bubble_x = straight + radius + WIDTH // 2 + s(13)
bubble_r = s(13)
draw.ellipse(
    (bubble_x - bubble_r, middle - bubble_r, bubble_x + bubble_r, middle + bubble_r),
    fill=BACKGROUND,
    outline=LINE,
    width=s(9),
)
draw.line((bubble_x + bubble_r, middle, s(222), middle), fill=LINE, width=WIDTH)
draw.ellipse((s(222) - WIDTH // 2, middle - WIDTH // 2, s(222) + WIDTH // 2, middle + WIDTH // 2), fill=LINE)

out = Path(__file__).resolve().parent.parent / "media" / "icon.png"
image.resize((256, 256), Image.LANCZOS).save(out)
print(out)
