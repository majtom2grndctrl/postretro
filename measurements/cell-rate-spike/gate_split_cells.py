"""Classify the cell-vs-per-fragment differences gate_captures.py found.

usage: uv run --no-project --with pillow --with numpy --with scipy python
       measurements/cell-rate-spike/gate_split_cells.py [view ...]

Reads runs/gate-<view>-frag_live.png and -cell_live.png (gate_captures.py keeps
them). In the cell image every water cell is one flat block, so a connected
region of one exact colour there is one cell (or a few equal neighbours). For
each region holding a pixel that differs by more than 2/255, count how many of
its pixels the per-fragment image still matches within 2/255:
- `split`: some do. Per-fragment pixel style itself drew this cell in more than
  one value (its derivative-estimated cell centre varies per fragment, and the
  search or shadow fade is discontinuous inside the cell), and the cell buffer
  drew it once, as one of those values;
- `whole`: none do. The cell's single value differs between the paths (the
  exact affine centre against the Jacobian estimate, or the analytic texels per
  pixel at a fade threshold).
Writes gate-split-cells.json beside this script.
"""
import json, sys
from pathlib import Path

import numpy as np
from PIL import Image
from scipy import ndimage

HERE = Path(__file__).resolve().parent


def classify(view):
    a = np.asarray(Image.open(HERE / "runs" / f"gate-{view}-frag_live.png").convert("RGB")).astype(np.int32)
    b = np.asarray(Image.open(HERE / "runs" / f"gate-{view}-cell_live.png").convert("RGB")).astype(np.int32)
    d = np.abs(a - b).max(axis=2)
    over = d > 2
    key = (b[..., 0] << 16) | (b[..., 1] << 8) | b[..., 2]
    split = whole = split_px = whole_px = 0
    seen = set()
    for y, x in zip(*np.nonzero(over)):
        if (y, x) in seen:
            continue
        # The equal-colour region of the cell image around this pixel, in a
        # 64-pixel window (a cell here is at most ~25 px across).
        y0, y1, x0, x1 = max(0, y - 64), y + 65, max(0, x - 64), x + 65
        win = key[y0:y1, x0:x1] == key[y, x]
        labels, _ = ndimage.label(win)
        region = labels == labels[y - y0, x - x0]
        ry, rx = np.nonzero(region)
        ry, rx = ry + y0, rx + x0
        matches = (d[ry, rx] <= 2).sum()
        n_over = over[ry, rx].sum()
        for p in zip(ry[over[ry, rx]], rx[over[ry, rx]]):
            seen.add(p)
        if matches > 0:
            split += 1
            split_px += int(n_over)
        else:
            whole += 1
            whole_px += int(n_over)
    return {"pixels_over_2": int(over.sum()), "max_diff": int(d.max()),
            "split_regions": split, "split_pixels": split_px,
            "whole_regions": whole, "whole_pixels": whole_px}


if __name__ == "__main__":
    views = sys.argv[1:] or ["near-4k", "near-1080p", "far-4k", "far-1080p"]
    out = {v: classify(v) for v in views}
    (HERE / "gate-split-cells.json").write_text(json.dumps(out, indent=2) + "\n")
    print(json.dumps(out, indent=2))
