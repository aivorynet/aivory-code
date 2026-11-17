"""Make the MSIX logo set from one square icon. Usage: make-logos.py <icon.png> <out-dir>"""
import sys
from pathlib import Path

from PIL import Image

src = Image.open(sys.argv[1]).convert("RGBA")
out = Path(sys.argv[2])
out.mkdir(parents=True, exist_ok=True)

SQUARES = {
    "StoreLogo.png": 50,
    "Square44x44Logo.png": 44,
    "Square44x44Logo.targetsize-256.png": 256,
    "Square150x150Logo.png": 150,
}
for name, size in SQUARES.items():
    src.resize((size, size), Image.LANCZOS).save(out / name)

wide = Image.new("RGBA", (310, 150), (0, 0, 0, 0))
icon = src.resize((120, 120), Image.LANCZOS)
wide.paste(icon, ((310 - 120) // 2, 15), icon)
wide.save(out / "Wide310x150Logo.png")
