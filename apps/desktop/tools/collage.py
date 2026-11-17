"""Detail collage for the judge: crops of several screenshots, labelled, in one image.

Usage: python -I collage.py <out.jpg> <png>:<x>,<y>,<w>,<h>[:<label>] ...
Coordinates are fractions (0-1) of the image size, so one crop spec fits every window size.
Each crop is scaled to 960 px wide; 2 columns.
"""
import sys
from PIL import Image, ImageDraw, ImageFont

out, specs = sys.argv[1], sys.argv[2:]
tiles = []
for spec in specs:
    parts = spec.rsplit(":", 2) if spec.count(":") >= 3 else spec.rsplit(":", 1)
    path, box = parts[0], parts[1]
    label = parts[2] if len(parts) > 2 else path.replace("\\", "/").rsplit("/", 1)[-1]
    im = Image.open(path).convert("RGB")
    x, y, w, h = (float(v) for v in box.split(","))
    W, H = im.size
    c = im.crop((int(x * W), int(y * H), int((x + w) * W), int((y + h) * H)))
    c = c.resize((960, max(1, int(c.height * 960 / c.width))), Image.LANCZOS)
    tiles.append((label, c))
band, cols = 28, 2
rows = [tiles[i:i + cols] for i in range(0, len(tiles), cols)]
hs = [max(t[1].height for t in r) + band for r in rows]
sheet = Image.new("RGB", (cols * 960 + 8, sum(hs)), (40, 40, 40))
d = ImageDraw.Draw(sheet)
try:
    font = ImageFont.truetype("segoeuib.ttf", 18)
except OSError:
    font = ImageFont.load_default()
y = 0
for r, h in zip(rows, hs):
    for i, (label, c) in enumerate(r):
        x = i * 968
        d.text((x + 6, y + 4), label, fill=(255, 255, 255), font=font)
        sheet.paste(c, (x, y + band))
    y += h
sheet.save(out, quality=85)
