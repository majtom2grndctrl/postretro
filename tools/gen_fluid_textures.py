#!/usr/bin/env python3
"""
Generate the dev mod's tileable fluid surface textures.

Writes `<out>/<name>.png` for each fluid below: water, sludge, lava, slime.
Every texture is built on a periodic domain (wrapping value noise, wrapping
Worley cells, toroidal bubble distances, a 4x4 ordered dither), so it tiles
seamlessly and can scroll or shimmer without seams. Each one is quantized to a
small palette so single-texel offsets stay visible under nearest sampling.
Standard library only (no Pillow); fixed seeds, so output is deterministic.

    python3 tools/gen_fluid_textures.py --out content/dev/textures/fluids

`--preview DIR` also writes a 2x2-tiled, 4x-upscaled copy of each texture
there, for checking seams by eye.
"""

import argparse
import math
import random
import struct
import zlib
from pathlib import Path

BAYER4 = [
    [0, 8, 2, 10],
    [12, 4, 14, 6],
    [3, 11, 1, 9],
    [15, 7, 13, 5],
]


def hex_palette(*colors):
    return [tuple(int(c[i:i + 2], 16) for i in (0, 2, 4)) for c in colors]


def smoothstep(t):
    return t * t * (3.0 - 2.0 * t)


def clamp01(t):
    return 0.0 if t < 0.0 else 1.0 if t > 1.0 else t


class ValueNoise:
    """Value noise on a `period` x `period` lattice that wraps in both axes."""

    def __init__(self, period, rng):
        self.period = period
        self.grid = [[rng.random() for _ in range(period)] for _ in range(period)]

    def sample(self, u, v):
        """Sample at (u, v) in [0, 1) texture space; wraps outside it."""
        p = self.period
        x, y = (u % 1.0) * p, (v % 1.0) * p
        x0, y0 = int(x), int(y)
        fx, fy = smoothstep(x - x0), smoothstep(y - y0)
        x0, y0 = x0 % p, y0 % p
        x1, y1 = (x0 + 1) % p, (y0 + 1) % p
        g = self.grid
        top = g[y0][x0] + (g[y0][x1] - g[y0][x0]) * fx
        bottom = g[y1][x0] + (g[y1][x1] - g[y1][x0]) * fx
        return top + (bottom - top) * fy


class Fbm:
    """Octaves of wrapping value noise; each octave doubles the lattice period."""

    def __init__(self, base_period, octaves, rng, gain=0.5):
        self.layers = [ValueNoise(base_period << i, rng) for i in range(octaves)]
        self.gain = gain

    def sample(self, u, v):
        total, amp, norm = 0.0, 1.0, 0.0
        for layer in self.layers:
            total += layer.sample(u, v) * amp
            norm += amp
            amp *= self.gain
        return total / norm


class Worley:
    """Jittered cellular noise on an `n` x `n` grid of cells that wraps."""

    def __init__(self, n, rng, jitter=0.85):
        self.n = n
        self.points = [
            [(0.5 + (rng.random() - 0.5) * jitter, 0.5 + (rng.random() - 0.5) * jitter)
             for _ in range(n)]
            for _ in range(n)
        ]

    def sample(self, u, v):
        """Return (F1, F2) distances in cell units at (u, v)."""
        n = self.n
        x, y = (u % 1.0) * n, (v % 1.0) * n
        cx, cy = int(x), int(y)
        f1 = f2 = 9.0
        for dy in (-1, 0, 1):
            for dx in (-1, 0, 1):
                gx, gy = cx + dx, cy + dy
                px, py = self.points[gy % n][gx % n]
                d = math.hypot(gx + px - x, gy + py - y)
                if d < f1:
                    f1, f2 = d, f1
                elif d < f2:
                    f2 = d
        return f1, f2


def wrap_delta(a, b):
    """Shortest signed distance from b to a on the unit circle."""
    d = (a - b) % 1.0
    return d - 1.0 if d > 0.5 else d


class Image:
    def __init__(self, size):
        self.size = size
        self.pixels = [[(0, 0, 0)] * size for _ in range(size)]

    def png(self):
        raw = bytearray()
        for row in self.pixels:
            raw.append(0)
            for r, g, b in row:
                raw += bytes((r, g, b, 255))

        def chunk(kind, data):
            body = kind + data
            return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

        header = struct.pack(">IIBBBBB", self.size, self.size, 8, 6, 0, 0, 0)
        return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
                + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b""))

    def tiled_preview(self, tiles=2, scale=4):
        out = Image(self.size * tiles * scale)
        for y in range(out.size):
            src = self.pixels[(y // scale) % self.size]
            out.pixels[y] = [src[(x // scale) % self.size] for x in range(out.size)]
        return out


def render(size, palette, field, dither=0.6, overrides=None):
    """Quantize a scalar field in [0, 1] onto `palette` with a 4x4 ordered dither.

    `field(u, v)` is sampled at texel centers. `overrides(x, y)` may return a
    palette index that replaces the quantized value (bubbles, sparks).
    """
    img = Image(size)
    n = len(palette)
    for y in range(size):
        row = []
        for x in range(size):
            u, v = (x + 0.5) / size, (y + 0.5) / size
            t = clamp01(field(u, v))
            bias = (BAYER4[y % 4][x % 4] + 0.5) / 16.0 - 0.5
            index = int(t * n + bias * dither)
            index = max(0, min(n - 1, index))
            if overrides is not None:
                forced = overrides(x, y)
                if forced is not None:
                    index = forced
            row.append(palette[index])
        img.pixels[y] = row
    return img


def scatter_bubbles(rng, count, radii, size):
    """Bubble centers on texel centers, with whole-texel radii drawn from `radii`."""
    return [(rng.randrange(size), rng.randrange(size), rng.choice(radii))
            for _ in range(count)]


def bubble_overrides(bubbles, size, rim, hilite, inner=None):
    """Draw bubbles as a 1-texel pixel-circle rim with a highlight texel up-left.

    Offsets wrap around the texture, so bubbles straddling an edge tile cleanly.
    """
    half = size // 2

    def lookup(x, y):
        for bx, by, r in bubbles:
            dx = (x - bx + half) % size - half
            dy = (y - by + half) % size - half
            d = math.hypot(dx, dy)
            if d >= r + 0.5:
                continue
            if r >= 2 and dx == -1 and dy == -1:
                return hilite
            if d >= r - 0.5:
                return rim
            if inner is not None:
                return inner
        return None
    return lookup


# --- fluids -----------------------------------------------------------------

WATER = hex_palette(
    "050b1c", "081633", "0b2147", "0f2f5c", "123f70", "155083",
    "186394", "1c78a4", "2390b3", "34aac2", "58c8d4", "9ee8ea", "e2fffb",
)

WATER_NEON = hex_palette(
    "0a0618", "140a2c", "1f0e40", "2c1155", "3d1368", "541579",
    "6e1a86", "8a2190", "a62c97", "1c5c9a", "2392b8", "3fd0d8", "c8fff8",
)

SLUDGE = hex_palette(
    "0e0c06", "19170a", "25230d", "323010", "403c12", "4e4814",
    "5d5418", "6c601c", "7d6e24", "908030", "a89a48", "c8bc70",
)

LAVA = hex_palette(
    "0e0404", "1c0705", "2c0a06", "420e07", "5c1308", "7a1a08",
    "a02608", "c83a0a", "e85a0c", "ff8414", "ffb22a", "ffe060", "fff6b8",
)

SLIME = hex_palette(
    "04100a", "07200f", "0a3012", "0e4214", "135516", "1a6a17",
    "238218", "329a1a", "48b41e", "66cc2a", "8ee43e", "c4fa70", "f0ffc8",
)


def water(size, seed, cells, palette):
    rng = random.Random(seed)
    swell = Fbm(4, 3, rng)
    warp_u = Fbm(4, 2, rng)
    warp_v = Fbm(4, 2, rng)
    caustic = Worley(cells, rng)
    caustic_fine = Worley(cells * 2, rng)

    def field(u, v):
        wu = u + (warp_u.sample(u, v) - 0.5) * 0.18
        wv = v + (warp_v.sample(u, v) - 0.5) * 0.18
        f1, f2 = caustic.sample(wu, wv)
        g1, g2 = caustic_fine.sample(wu + 0.37, wv + 0.11)
        # Bright caustic web where two cells meet; a fainter fine web on top.
        web = 1.0 - clamp01((f2 - f1) / 0.22)
        fine = 1.0 - clamp01((g2 - g1) / 0.18)
        base = swell.sample(u, v)
        return 0.08 + base * 0.48 + web * web * 0.55 + fine * 0.14

    return render(size, palette, field, dither=0.9)


def sludge(size, seed):
    rng = random.Random(seed)
    warp_a = Fbm(3, 3, rng)
    warp_b = Fbm(3, 3, rng)
    body = Fbm(4, 3, rng)
    scale = size / 64.0

    def field(u, v):
        wu = u + (warp_a.sample(u, v) - 0.5) * 0.35
        wv = v + (warp_b.sample(u + 0.5, v) - 0.5) * 0.35
        b = body.sample(wu, wv)
        # Oily sheen lines along the contours of a warped, lumpy body.
        sheen = 0.5 + 0.5 * math.sin(b * 40.0)
        return 0.05 + b * 0.50 + sheen ** 6 * 0.40

    bubbles = scatter_bubbles(rng, int(9 * scale * scale), (2, 2, 3), size)
    overrides = bubble_overrides(bubbles, size, rim=9, hilite=11)
    return render(size, SLUDGE, field, dither=0.9, overrides=overrides)


def lava(size, seed):
    rng = random.Random(seed)
    plates = Worley(max(2, size // 13), rng, jitter=0.95)
    warp_u = Fbm(4, 2, rng)
    warp_v = Fbm(4, 2, rng)
    heat = Fbm(4, 3, rng)
    crust = Fbm(8, 2, rng)

    def field(u, v):
        wu = u + (warp_u.sample(u, v) - 0.5) * 0.12
        wv = v + (warp_v.sample(u, v) - 0.5) * 0.12
        f1, f2 = plates.sample(wu, wv)
        crack = 1.0 - clamp01((f2 - f1) / 0.30)
        # Dark crust plates, mottled; cracks between them glow molten.
        plate = crust.sample(u, v) * 0.30 + clamp01(f1 - 0.15) * 0.25
        glow = heat.sample(u, v)
        return plate * (1.0 - crack) + crack * (0.55 + glow * 0.5)

    bubbles = scatter_bubbles(rng, int(4 * (size / 64.0) ** 2), (1, 2), size)
    overrides = bubble_overrides(bubbles, size, rim=9, hilite=12, inner=11)
    return render(size, LAVA, field, dither=0.9, overrides=overrides)


def slime(size, seed):
    rng = random.Random(seed)
    blobs = Worley(max(2, size // 11), rng)
    warp_u = Fbm(3, 2, rng)
    warp_v = Fbm(3, 2, rng)
    body = Fbm(4, 3, rng)

    def field(u, v):
        wu = u + (warp_u.sample(u, v) - 0.5) * 0.22
        wv = v + (warp_v.sample(u, v) - 0.5) * 0.22
        f1, _ = blobs.sample(wu, wv)
        # Glowing gel blobs (bright cores) in a darker, mottled body.
        core = clamp01(1.0 - f1 * 1.5)
        return 0.04 + body.sample(u, v) * 0.45 + core * core * 0.65

    bubbles = scatter_bubbles(rng, int(7 * (size / 64.0) ** 2), (2, 2, 3), size)
    overrides = bubble_overrides(bubbles, size, rim=11, hilite=12, inner=None)
    return render(size, SLIME, field, dither=0.9, overrides=overrides)


FLUIDS = {
    "water_64": lambda: water(64, 1101, 5, WATER),
    "water_128": lambda: water(128, 1102, 10, WATER),
    "water_neon_64": lambda: water(64, 1103, 5, WATER_NEON),
    "sludge_64": lambda: sludge(64, 2201),
    "lava_64": lambda: lava(64, 3301),
    "slime_64": lambda: slime(64, 4401),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[1])
    parser.add_argument("--out", type=Path, required=True, help="output directory for the PNGs")
    parser.add_argument("--preview", type=Path, help="also write 2x2-tiled 4x previews here")
    args = parser.parse_args()

    args.out.mkdir(parents=True, exist_ok=True)
    if args.preview:
        args.preview.mkdir(parents=True, exist_ok=True)
    for name, build in FLUIDS.items():
        img = build()
        (args.out / f"{name}.png").write_bytes(img.png())
        if args.preview:
            (args.preview / f"{name}_tiled.png").write_bytes(img.tiled_preview().png())
        print(f"wrote {name}.png ({img.size}x{img.size})")


if __name__ == "__main__":
    main()
