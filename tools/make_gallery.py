#!/usr/bin/env python3
"""Turn the gallery artwork into one raw image, linked into the firmware.

Called by `build.rs`, which embeds the result in the `.sfc_xip` section -- so
the pictures are part of the ELF, at an address inside the SFC window, and are
never copied into the 512K of internal flash.

Layout (little endian):

    offset 0    : b"SWRA" | u16 version | u16 count | u32 reserved
    offset 16   : count * 16-byte entries
                    u32 offset (from the start of this image)
                    u32 size   (bytes of pixel data)
                    u16 width, u16 height
                    u8  format (0 = RGB565) + 3 pad bytes
    offset 4096 : big-endian RGB565 pixel data, every picture 4-byte aligned

`assets/*.png|jpg` is centre-cropped and scaled to --size; with no artwork
present three synthetic pictures are generated so the demo runs out of the box.
"""

import argparse
import os
import struct
import sys

try:
    from PIL import Image
except ImportError:
    Image = None

FORMAT_RGB565 = 0
TABLE_SIZE = 4096          # the table owns the first sector
HEADER_SIZE = 16
ALIGN = 4


def rgb888_to_rgb565(buf):
    """bytes of R,G,B,R,G,B... -> bytes of big-endian RGB565."""
    out = bytearray()
    for i in range(0, len(buf), 3):
        r, g, b = buf[i], buf[i + 1], buf[i + 2]
        out += struct.pack(">H", ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3))
    return bytes(out)


def synthetic_pictures(width, height):
    """Three pictures drawn without Pillow: gradients, rings, checkerboard."""
    pictures = []
    for kind in range(3):
        buf = bytearray()
        for y in range(height):
            for x in range(width):
                if kind == 0:
                    r = x * 255 // width
                    g = y * 255 // height
                    b = ((x + y) * 255) // (width + height)
                elif kind == 1:
                    cx, cy = width // 2, height // 2
                    d = int(((x - cx) ** 2 + (y - cy) ** 2) ** 0.5)
                    ring = (d // 18) % 2
                    r = 240 if ring else 20
                    g = 120 if ring else 40
                    b = 60 if ring else 160
                else:
                    cell = ((x // 20) + (y // 20)) % 2
                    r, g, b = (235, 235, 235) if cell else (30, 90, 140)
                buf += bytes((r, g, b))
        pictures.append(bytes(buf))
    return pictures


def background_rgb(background):
    return tuple(int(background[i:i + 2], 16) for i in (0, 2, 4))


def flatten(im, background):
    """Composite a possibly transparent image onto `background` (#RRGGBB).

    The panel has no alpha channel to show, so dropping it outright would leave
    the transparent parts of e.g. an SVG in whatever colour the pixels happen to
    carry -- usually black.
    """
    if im.mode not in ("RGBA", "LA", "PA"):
        return im.convert("RGB")
    base = Image.new("RGB", im.size, background_rgb(background))
    base.paste(im, mask=im.convert("RGBA").split()[3])
    return base


def load_pictures(dirs, width, height, background):
    """Every bitmap in `dirs`, centre-cropped to the target aspect and scaled."""
    if Image is None:
        print("no Pillow -- falling back to synthetic pictures only")
        return [], []

    files = []
    for directory in dirs:
        if not os.path.isdir(directory):
            continue
        files += [
            (directory, f) for f in sorted(os.listdir(directory))
            if f.lower().endswith((".png", ".jpg", ".jpeg", ".bmp"))
        ]

    pictures, names = [], []
    for directory, name in files:
        im = Image.open(os.path.join(directory, name))
        im = flatten(im, background)
        # Scale to fit and centre on a full-size canvas ("letterbox") rather
        # than cropping: artwork is shown whole, whatever its aspect.
        scale = min(width / im.width, height / im.height)
        scaled = im.resize(
            (max(1, int(im.width * scale)), max(1, int(im.height * scale))),
            Image.LANCZOS,
        )
        canvas = Image.new("RGB", (width, height), background_rgb(background))
        canvas.paste(
            scaled, ((width - scaled.width) // 2, (height - scaled.height) // 2)
        )
        pictures.append(canvas.tobytes())
        names.append(name)
    return pictures, names


def main():
    ap = argparse.ArgumentParser()
    # Repeatable: `assets/` for bitmaps, plus the directory build.rs fills with
    # the rasterised SVGs (which are not bitmaps and need resvg first).
    ap.add_argument("--assets", action="append", default=[])
    ap.add_argument("--out", required=True, help="path of the image to write")
    ap.add_argument("--size", default="480x320", help="WxH of every picture")
    ap.add_argument("--background", default="FFFFFF",
                    help="#RRGGBB to composite transparent artwork onto")
    ap.add_argument("--synthetic", type=int, default=3,
                    help="extra generated pictures, so the gallery is never empty")
    args = ap.parse_args()

    width, height = (int(v) for v in args.size.split("x"))
    pictures, names = load_pictures(args.assets, width, height, args.background)

    for i, picture in enumerate(synthetic_pictures(width, height)[:args.synthetic]):
        pictures.append(picture)
        names.append("synthetic-%d" % i)

    blob = bytearray(b"SWRA" + struct.pack("<HHI", 1, len(pictures), 0))
    blob += bytes(HEADER_SIZE - len(blob))
    data = bytearray()
    entries = []
    for picture, name in zip(pictures, names):
        pixels = rgb888_to_rgb565(picture)
        entries.append((TABLE_SIZE + len(data), len(pixels), width, height, name))
        data += pixels
        while len(data) % ALIGN:
            data += b"\0"

    for offset, size, w, h, _ in entries:
        blob += struct.pack("<IIHHBBBB", offset, size, w, h, FORMAT_RGB565, 0, 0, 0)
    blob += bytes(TABLE_SIZE - len(blob))
    blob += data

    os.makedirs(os.path.dirname(args.out) or ".", exist_ok=True)
    with open(args.out, "wb") as f:
        f.write(blob)

    print(
        "gallery: %d bytes, %d pictures, %dx%d RGB565"
        % (len(blob), len(pictures), width, height)
    )
    for offset, size, w, h, name in entries:
        print("  %-20s %7d bytes @ 0x%06X" % (name, size, offset))
    return 0


if __name__ == "__main__":
    sys.exit(main())
