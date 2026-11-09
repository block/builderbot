#!/usr/bin/env python3
"""Build the full-bleed home-screen web app icons from the macOS Icon Composer render.

The macOS render (src-tauri/icons/Staged.png, 1024 px) carries the macOS icon shape:
transparent rounded corners and a grey bevel along the shape's edge. iOS masks
apple-touch-icon with its own squircle and composites transparency onto black, so
that edge treatment shows as a light rim with mismatched corners on the home screen.
This keeps the glass-rendered lines and the subtle background gradient and only
replaces the edge bevel and the transparent corners with the background colour of the
same row, giving an opaque full-bleed square for iOS to mask.

usage: make-web-icons.py <Staged.png> <out dir>
"""
import sys
from PIL import Image, ImageFilter, ImageOps
from statistics import median

EDGE_PX = 30          # width of the edge zone to flatten, at 1024 px
GREY_CHROMA = 12      # max(R,G,B)-min(R,G,B) below this counts as background, not a line

src, out = sys.argv[1], sys.argv[2]
im = Image.open(src).convert('RGBA')
w, h = im.size
px = im.load()
shape = im.getchannel('A').point(lambda v: 255 if v > 250 else 0)
# pad with 'outside' first so the straight image edges erode too (PIL replicates edge pixels when filtering)
interior = ImageOps.expand(shape, EDGE_PX, 0).filter(ImageFilter.MinFilter(EDGE_PX * 2 + 1)).crop((EDGE_PX, EDGE_PX, EDGE_PX + w, EDGE_PX + h))
ipx = interior.load()

# background grey per row, from the interior pixels of that row (the render has a faint vertical gradient)
row_bg = {}
for y in range(h):
    greys = [px[x, y][0] for x in range(w) if ipx[x, y] and max(px[x, y][:3]) - min(px[x, y][:3]) < GREY_CHROMA]
    row_bg[y] = int(median(greys)) if greys else None
known = [y for y in range(h) if row_bg[y] is not None]
for y in range(h):
    if row_bg[y] is None:
        row_bg[y] = row_bg[min(known, key=lambda k: abs(k - y))]

result = Image.new('RGB', (w, h))
rpx = result.load()
for y in range(h):
    bg = (row_bg[y],) * 3
    for x in range(w):
        r, g, b, a = px[x, y]
        if ipx[x, y]:
            rpx[x, y] = (r, g, b)
        elif a > 0 and max(r, g, b) - min(r, g, b) >= GREY_CHROMA:
            # a glass-rendered line crossing the edge zone: keep it, flattening any transparency onto the background
            f = a / 255
            rpx[x, y] = tuple(round(c * f + bg[0] * (1 - f)) for c in (r, g, b))
        else:
            rpx[x, y] = bg
result.save(f'{out}/staged-1024-flat.png')
for size, name in ((180, 'apple-touch-icon'), (192, 'staged-192'), (512, 'staged-512')):
    result.resize((size, size), Image.LANCZOS).save(f'{out}/{name}.png', optimize=True)
    print(f'{out}/{name}.png')
