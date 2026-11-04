#!/usr/bin/env python3
"""Labelled verdict sheet PNG, standard library only: verdict_sheet.py <capture dir> <out.png>."""
import os, struct, sys, zlib
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pngstat

GLYPHS = {
    "A": "01110 10001 10001 11111 10001 10001 10001", "D": "11110 10001 10001 10001 10001 10001 11110",
    "E": "11111 10000 10000 11110 10000 10000 11111", "G": "01110 10001 10000 10111 10001 10001 01111",
    "I": "01110 00100 00100 00100 00100 00100 01110", "L": "10000 10000 10000 10000 10000 10000 11111",
    "N": "10001 11001 10101 10011 10001 10001 10001", "O": "01110 10001 10001 10001 10001 10001 01110",
    "R": "11110 10001 10001 11110 10100 10010 10001", "S": "01111 10000 10000 01110 00001 00001 11110",
    "T": "11111 00100 00100 00100 00100 00100 00100", "U": "10001 10001 10001 10001 10001 10001 01110",
    "X": "10001 10001 01010 00100 01010 10001 10001", "Y": "10001 10001 01010 00100 00100 00100 00100",
    "2": "01110 10001 00001 00110 01000 10000 11111", "3": "11110 00001 00001 01110 00001 00001 11110",
    "4": "00010 00110 01010 10010 11111 00010 00010", "6": "01110 10000 10000 11110 10001 10001 01110",
    "8": "01110 10001 10001 01110 10001 10001 01110", " ": "00000 00000 00000 00000 00000 00000 00000",
}
W, H, HEAD, SIDE, SCALE = 480, 270, 34, 150, 3
COLS = [("TODAY", "off-static"), ("4X4", "static-t4-r256"), ("6X6", "static-t6-r256"), ("8X8", "static-t8-r256")]
ROWS = [("STATION 3", "threshold", 1), ("STATION 3 8X", "threshold", 8), ("STATION 2", "door", 1), ("STATION 2 8X", "door", 8)]


def text(canvas, s, x, y, scale=SCALE):
    for ch in s:
        for gy, row in enumerate(GLYPHS[ch].split()):
            for gx, bit in enumerate(row):
                if bit == "1":
                    for dy in range(scale):
                        for dx in range(scale):
                            o = (x + gx * scale + dx) * 3
                            canvas[y + gy * scale + dy][o:o + 3] = b"\xe0\xe0\xe0"
        x += 6 * scale


def half(img):
    """2x2 box downsample, so a thin outline dims rather than vanishes."""
    w, h, ch, rows = img
    out = []
    for y in range(H):
        a, b = rows[y * 2], rows[y * 2 + 1]
        line = [0] * (W * 3)
        for x in range(W):
            i, j = x * 2 * ch, (x * 2 + 1) * ch
            for c in range(3):
                line[x * 3 + c] = (a[i + c] + a[j + c] + b[i + c] + b[j + c]) / 4
        out.append(line)
    return out


def main():
    d, out = sys.argv[1], sys.argv[2]
    width, height = SIDE + len(COLS) * W, HEAD + len(ROWS) * H
    canvas = [bytearray(b"\x18" * (width * 3)) for _ in range(height)]
    for ci, (label, _) in enumerate(COLS):
        text(canvas, label, SIDE + ci * W + 8, 6)
    for ri, (label, view, gain) in enumerate(ROWS):
        y0 = HEAD + ri * H
        for word_i, word in enumerate(label.split(" ")):
            text(canvas, word, 6, y0 + 8 + word_i * 28, 2 if len(word) > 7 else SCALE)
        for ci, (_, stem) in enumerate(COLS):
            tile = half(pngstat.read_png(os.path.join(d, f"{stem}_{view}.png")))
            x0 = (SIDE + ci * W) * 3
            for y, line in enumerate(tile):
                canvas[y0 + y][x0:x0 + W * 3] = bytes(min(255, int(v * gain)) for v in line)
    raw = b"".join(b"\0" + bytes(r) for r in canvas)

    def chunk(t, body):
        return struct.pack(">I", len(body)) + t + body + struct.pack(">I", zlib.crc32(t + body))

    png = (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
           + chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b""))
    open(out, "wb").write(png)


main()
