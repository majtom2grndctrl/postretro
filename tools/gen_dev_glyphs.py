#!/usr/bin/env python3
"""
Generate the dev mod's placeholder input glyph art.

Writes `<out>/{kbm,xbox,ps,nx}/<input>.png`, one PNG per input name the engine
knows (`KeyW`, `mouse_left`, `south`, `left_stick_up`, ...). Standard library
only: shapes and a 5x7 bitmap font drawn on a supersampled canvas, encoded with
zlib. The art is functional placeholder art; a game ships its own.

    python3 tools/gen_dev_glyphs.py --out content/dev/ui/glyphs
"""

import argparse
import math
import struct
import zlib
from pathlib import Path

SIZE = 48  # Output height in pixels; keycaps grow wider for long labels.
SS = 4  # Supersampling factor.

FONT = {
    "A": ["01110", "10001", "10001", "11111", "10001", "10001", "10001"],
    "B": ["11110", "10001", "10001", "11110", "10001", "10001", "11110"],
    "C": ["01110", "10001", "10000", "10000", "10000", "10001", "01110"],
    "D": ["11110", "10001", "10001", "10001", "10001", "10001", "11110"],
    "E": ["11111", "10000", "10000", "11110", "10000", "10000", "11111"],
    "F": ["11111", "10000", "10000", "11110", "10000", "10000", "10000"],
    "G": ["01110", "10001", "10000", "10111", "10001", "10001", "01111"],
    "H": ["10001", "10001", "10001", "11111", "10001", "10001", "10001"],
    "I": ["01110", "00100", "00100", "00100", "00100", "00100", "01110"],
    "J": ["00111", "00010", "00010", "00010", "00010", "10010", "01100"],
    "K": ["10001", "10010", "10100", "11000", "10100", "10010", "10001"],
    "L": ["10000", "10000", "10000", "10000", "10000", "10000", "11111"],
    "M": ["10001", "11011", "10101", "10101", "10001", "10001", "10001"],
    "N": ["10001", "10001", "11001", "10101", "10011", "10001", "10001"],
    "O": ["01110", "10001", "10001", "10001", "10001", "10001", "01110"],
    "P": ["11110", "10001", "10001", "11110", "10000", "10000", "10000"],
    "Q": ["01110", "10001", "10001", "10001", "10101", "10010", "01101"],
    "R": ["11110", "10001", "10001", "11110", "10100", "10010", "10001"],
    "S": ["01111", "10000", "10000", "01110", "00001", "00001", "11110"],
    "T": ["11111", "00100", "00100", "00100", "00100", "00100", "00100"],
    "U": ["10001", "10001", "10001", "10001", "10001", "10001", "01110"],
    "V": ["10001", "10001", "10001", "10001", "10001", "01010", "00100"],
    "W": ["10001", "10001", "10001", "10101", "10101", "10101", "01010"],
    "X": ["10001", "10001", "01010", "00100", "01010", "10001", "10001"],
    "Y": ["10001", "10001", "01010", "00100", "00100", "00100", "00100"],
    "Z": ["11111", "00001", "00010", "00100", "01000", "10000", "11111"],
    "0": ["01110", "10001", "10011", "10101", "11001", "10001", "01110"],
    "1": ["00100", "01100", "00100", "00100", "00100", "00100", "01110"],
    "2": ["01110", "10001", "00001", "00010", "00100", "01000", "11111"],
    "3": ["11111", "00010", "00100", "00010", "00001", "10001", "01110"],
    "4": ["00010", "00110", "01010", "10010", "11111", "00010", "00010"],
    "5": ["11111", "10000", "11110", "00001", "00001", "10001", "01110"],
    "6": ["00110", "01000", "10000", "11110", "10001", "10001", "01110"],
    "7": ["11111", "00001", "00010", "00100", "01000", "01000", "01000"],
    "8": ["01110", "10001", "10001", "01110", "10001", "10001", "01110"],
    "9": ["01110", "10001", "10001", "01111", "00001", "00010", "01100"],
    "+": ["00000", "00100", "00100", "11111", "00100", "00100", "00000"],
    "-": ["00000", "00000", "00000", "11111", "00000", "00000", "00000"],
    " ": ["00000"] * 7,
}

WHITE = (0.95, 0.96, 0.97, 1.0)
DARK = (0.09, 0.10, 0.13, 1.0)
EDGE = (0.62, 0.66, 0.72, 1.0)
DIM = (0.30, 0.33, 0.38, 1.0)


class Canvas:
    def __init__(self, width, height):
        self.w, self.h = width * SS, height * SS
        self.out_w, self.out_h = width, height
        self.px = [[0.0, 0.0, 0.0, 0.0] for _ in range(self.w * self.h)]

    def blend(self, x, y, color):
        if 0 <= x < self.w and 0 <= y < self.h:
            p = self.px[y * self.w + x]
            a = color[3]
            for i in range(3):
                p[i] = color[i] * a + p[i] * (1 - a)
            p[3] = a + p[3] * (1 - a)

    def fill(self, inside, color, box=None):
        x0, y0, x1, y1 = box or (0, 0, self.w, self.h)
        for y in range(max(0, int(y0)), min(self.h, int(y1) + 1)):
            for x in range(max(0, int(x0)), min(self.w, int(x1) + 1)):
                if inside(x + 0.5, y + 0.5):
                    self.blend(x, y, color)

    def circle(self, cx, cy, r, color):
        cx, cy, r = cx * SS, cy * SS, r * SS
        self.fill(lambda x, y: (x - cx) ** 2 + (y - cy) ** 2 <= r * r, color,
                  (cx - r, cy - r, cx + r, cy + r))

    def ring(self, cx, cy, r, thickness, color):
        cx, cy, r, t = cx * SS, cy * SS, r * SS, thickness * SS
        self.fill(lambda x, y: (r - t) ** 2 <= (x - cx) ** 2 + (y - cy) ** 2 <= r * r, color,
                  (cx - r, cy - r, cx + r, cy + r))

    def rrect(self, x0, y0, x1, y1, radius, color, thickness=None):
        x0, y0, x1, y1, rad = x0 * SS, y0 * SS, x1 * SS, y1 * SS, radius * SS

        def inside_box(x, y, a0, b0, a1, b1, rr):
            if not (a0 <= x <= a1 and b0 <= y <= b1):
                return False
            cx = min(max(x, a0 + rr), a1 - rr)
            cy = min(max(y, b0 + rr), b1 - rr)
            return (x - cx) ** 2 + (y - cy) ** 2 <= rr * rr

        if thickness is None:
            self.fill(lambda x, y: inside_box(x, y, x0, y0, x1, y1, rad), color, (x0, y0, x1, y1))
        else:
            t = thickness * SS
            self.fill(
                lambda x, y: inside_box(x, y, x0, y0, x1, y1, rad)
                and not inside_box(x, y, x0 + t, y0 + t, x1 - t, y1 - t, max(rad - t, 0)),
                color,
                (x0, y0, x1, y1),
            )

    def polygon(self, points, color):
        pts = [(x * SS, y * SS) for x, y in points]

        def inside(x, y):
            hit = False
            j = len(pts) - 1
            for i in range(len(pts)):
                xi, yi = pts[i]
                xj, yj = pts[j]
                if (yi > y) != (yj > y) and x < (xj - xi) * (y - yi) / (yj - yi) + xi:
                    hit = not hit
                j = i
            return hit

        xs = [p[0] for p in pts]
        ys = [p[1] for p in pts]
        self.fill(inside, color, (min(xs), min(ys), max(xs), max(ys)))

    def line(self, x0, y0, x1, y1, thickness, color):
        ax, ay, bx, by, t = x0 * SS, y0 * SS, x1 * SS, y1 * SS, thickness * SS / 2

        def inside(x, y):
            dx, dy = bx - ax, by - ay
            k = max(0.0, min(1.0, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy)))
            return (x - ax - k * dx) ** 2 + (y - ay - k * dy) ** 2 <= t * t

        self.fill(inside, color, (min(ax, bx) - t, min(ay, by) - t, max(ax, bx) + t, max(ay, by) + t))

    def text(self, label, cx, cy, max_width, max_height, color):
        """Centered bitmap text, scaled to fit the box (in output pixels)."""
        cols = len(label) * 6 - 1
        scale = min(max_width * SS / cols, max_height * SS / 7, 3.0 * SS)
        scale = max(scale, SS * 0.75)
        x0 = cx * SS - cols * scale / 2
        y0 = cy * SS - 7 * scale / 2
        for i, ch in enumerate(label):
            rows = FONT.get(ch, FONT[" "])
            for r, row in enumerate(rows):
                for c, bit in enumerate(row):
                    if bit == "1":
                        # Whole canvas pixels, so neighbouring font pixels meet
                        # with no gap between them.
                        bx0 = round(x0 + (i * 6 + c) * scale)
                        by0 = round(y0 + r * scale)
                        bx1 = round(x0 + (i * 6 + c + 1) * scale)
                        by1 = round(y0 + (r + 1) * scale)
                        for y in range(max(0, by0), min(self.h, by1)):
                            for x in range(max(0, bx0), min(self.w, bx1)):
                                self.blend(x, y, color)

    def png(self):
        rows = []
        for oy in range(self.out_h):
            row = bytearray([0])
            for ox in range(self.out_w):
                acc = [0.0, 0.0, 0.0, 0.0]
                for sy in range(SS):
                    for sx in range(SS):
                        p = self.px[(oy * SS + sy) * self.w + ox * SS + sx]
                        a = p[3]
                        acc[0] += p[0] * a
                        acc[1] += p[1] * a
                        acc[2] += p[2] * a
                        acc[3] += a
                n = SS * SS
                alpha = acc[3] / n
                if alpha > 0:
                    rgb = [acc[i] / acc[3] for i in range(3)]
                else:
                    rgb = [0.0, 0.0, 0.0]
                row += bytes(int(round(max(0.0, min(1.0, v)) * 255)) for v in (*rgb, alpha))
            rows.append(bytes(row))
        raw = b"".join(rows)

        def chunk(kind, data):
            body = kind + data
            return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

        header = struct.pack(">IIBBBBB", self.out_w, self.out_h, 8, 6, 0, 0, 0)
        return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
                + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def arrow(canvas, cx, cy, direction, size, color):
    dx, dy = {"up": (0, -1), "down": (0, 1), "left": (-1, 0), "right": (1, 0)}[direction]
    px, py = -dy, dx
    tip = (cx + dx * size, cy + dy * size)
    base_l = (cx - dx * size * 0.4 + px * size * 0.8, cy - dy * size * 0.4 + py * size * 0.8)
    base_r = (cx - dx * size * 0.4 - px * size * 0.8, cy - dy * size * 0.4 - py * size * 0.8)
    canvas.polygon([tip, base_l, base_r], color)


# --- gamepad -----------------------------------------------------------------

FACE = {
    "xbox": {
        "south": ("A", (0.30, 0.72, 0.22, 1.0)),
        "east": ("B", (0.85, 0.22, 0.22, 1.0)),
        "west": ("X", (0.16, 0.45, 0.90, 1.0)),
        "north": ("Y", (0.90, 0.72, 0.12, 1.0)),
    },
    "nx": {
        "south": ("B", DARK),
        "east": ("A", DARK),
        "west": ("Y", DARK),
        "north": ("X", DARK),
    },
}
PS_FACE = {
    "south": ("cross", (0.48, 0.62, 0.96, 1.0)),
    "east": ("circle", (0.95, 0.38, 0.38, 1.0)),
    "west": ("square", (0.90, 0.52, 0.82, 1.0)),
    "north": ("triangle", (0.32, 0.86, 0.72, 1.0)),
}
SHOULDERS = {
    "xbox": {"left_shoulder": "LB", "right_shoulder": "RB", "left_trigger": "LT", "right_trigger": "RT",
             "select": "VIEW", "start": "MENU", "left_stick_press": "LS", "right_stick_press": "RS"},
    "ps": {"left_shoulder": "L1", "right_shoulder": "R1", "left_trigger": "L2", "right_trigger": "R2",
           "select": "SHR", "start": "OPT", "left_stick_press": "L3", "right_stick_press": "R3"},
    "nx": {"left_shoulder": "L", "right_shoulder": "R", "left_trigger": "ZL", "right_trigger": "ZR",
           "select": "-", "start": "+", "left_stick_press": "LS", "right_stick_press": "RS"},
}


def face_button(family, name):
    c = Canvas(SIZE, SIZE)
    if family == "ps":
        shape, color = PS_FACE[name]
        c.circle(24, 24, 22, DARK)
        c.ring(24, 24, 22, 2, EDGE)
        if shape == "cross":
            c.line(15, 15, 33, 33, 4, color)
            c.line(33, 15, 15, 33, 4, color)
        elif shape == "circle":
            c.ring(24, 24, 11, 4, color)
        elif shape == "square":
            c.rrect(14, 14, 34, 34, 1, color, thickness=4)
        else:
            c.polygon([(24, 12), (36, 33), (12, 33)], color)
            c.polygon([(24, 19), (30.5, 30), (17.5, 30)], DARK)
    else:
        letter, color = FACE[family][name]
        c.circle(24, 24, 22, color)
        c.ring(24, 24, 22, 2, EDGE)
        c.text(letter, 24, 24, 20, 22, WHITE)
    return c


def labeled_button(label, round_cap):
    c = Canvas(SIZE, SIZE)
    if round_cap:
        c.circle(24, 24, 22, DARK)
        c.ring(24, 24, 22, 2, EDGE)
    else:
        c.rrect(2, 10, 46, 38, 9, DARK)
        c.rrect(2, 10, 46, 38, 9, EDGE, thickness=2)
    c.text(label, 24, 24, 34, 16, WHITE)
    return c


def dpad(direction):
    c = Canvas(SIZE, SIZE)
    for d in ("up", "down", "left", "right"):
        dx, dy = {"up": (0, -1), "down": (0, 1), "left": (-1, 0), "right": (1, 0)}[d]
        x0, y0 = 24 + dx * 13 - 7, 24 + dy * 13 - 7
        c.rrect(x0, y0, x0 + 14, y0 + 14, 3, WHITE if d == direction else DIM)
    c.rrect(17, 17, 31, 31, 1, DIM)
    return c


def stick(side, direction=None, axis=None):
    c = Canvas(SIZE, SIZE)
    c.circle(24, 24, 22, DARK)
    c.ring(24, 24, 22, 2, EDGE)
    c.text(side, 24, 24, 10, 12, WHITE)
    dirs = [direction] if direction else (["left", "right"] if axis == "x" else ["up", "down"])
    for d in dirs:
        dx, dy = {"up": (0, -1), "down": (0, 1), "left": (-1, 0), "right": (1, 0)}[d]
        arrow(c, 24 + dx * 14, 24 + dy * 14, d, 5, WHITE)
    return c


def gamepad_set(family):
    art = {}
    for name in ("south", "east", "west", "north"):
        art[name] = face_button(family, name)
    for name, label in SHOULDERS[family].items():
        art[name] = labeled_button(label, round_cap=name.endswith("stick_press"))
    for d in ("up", "down", "left", "right"):
        art[f"dpad_{d}"] = dpad(d)
    for side, prefix in (("L", "left"), ("R", "right")):
        for d in ("up", "down", "left", "right"):
            art[f"{prefix}_stick_{d}"] = stick(side, direction=d)
        art[f"{prefix}_stick_x"] = stick(side, axis="x")
        art[f"{prefix}_stick_y"] = stick(side, axis="y")
    return art


# --- keyboard and mouse ------------------------------------------------------

NAMED_KEYS = {
    "Space": "SPACE", "Enter": "ENTER", "NumpadEnter": "ENTER", "Escape": "ESC", "Tab": "TAB",
    "ShiftLeft": "SHIFT", "ShiftRight": "SHIFT", "ControlLeft": "CTRL", "ControlRight": "CTRL",
    "AltLeft": "ALT", "AltRight": "ALT", "Backspace": "BKSP", "CapsLock": "CAPS",
    "Backquote": "`", "Minus": "-", "Equal": "+",
}


def keycap(label):
    width = SIZE if len(label) <= 2 else max(SIZE, len(label) * 9 + 18)
    c = Canvas(width, SIZE)
    c.rrect(2, 4, width - 2, 44, 7, DARK)
    c.rrect(2, 4, width - 2, 44, 7, EDGE, thickness=2)
    c.text(label, width / 2, 23, width - 16, 18, WHITE)
    return c


def arrow_key(direction):
    c = Canvas(SIZE, SIZE)
    c.rrect(2, 4, 46, 44, 7, DARK)
    c.rrect(2, 4, 46, 44, 7, EDGE, thickness=2)
    arrow(c, 24, 24, direction, 10, WHITE)
    return c


def mouse(part):
    c = Canvas(SIZE, SIZE)
    c.rrect(12, 4, 36, 44, 11, DARK)
    c.rrect(12, 4, 36, 44, 11, EDGE, thickness=2)
    c.line(24, 6, 24, 21, 1.5, EDGE)
    c.line(13, 21, 35, 21, 1.5, EDGE)
    if part == "left":
        c.polygon([(15, 9), (22.5, 6.5), (22.5, 19.5), (14, 19.5)], WHITE)
    elif part == "right":
        c.polygon([(25.5, 6.5), (33, 9), (34, 19.5), (25.5, 19.5)], WHITE)
    elif part in ("middle", "wheel_up", "wheel_down"):
        c.rrect(21.5, 9, 26.5, 18, 2.5, WHITE)
        if part != "middle":
            arrow(c, 24, 31 if part == "wheel_down" else 29, "down" if part == "wheel_down" else "up", 6, WHITE)
    elif part in ("back", "forward"):
        c.text("4" if part == "back" else "5", 24, 32, 8, 10, WHITE)
    elif part == "x":
        arrow(c, 5, 30, "left", 4, WHITE)
        arrow(c, 43, 30, "right", 4, WHITE)
    elif part == "y":
        arrow(c, 42, 24, "up", 4, WHITE)
        arrow(c, 42, 36, "down", 4, WHITE)
    return c


def kbm_set():
    art = {}
    for ch in "ABCDEFGHIJKLMNOPQRSTUVWXYZ":
        art[f"Key{ch}"] = keycap(ch)
    for d in "0123456789":
        art[f"Digit{d}"] = keycap(d)
    for name, label in NAMED_KEYS.items():
        art[name] = keycap(label)
    for d in ("Up", "Down", "Left", "Right"):
        art[f"Arrow{d}"] = arrow_key(d.lower())
    for part in ("left", "right", "middle", "back", "forward"):
        art[f"mouse_{part}"] = mouse(part)
    art["wheel_up"] = mouse("wheel_up")
    art["wheel_down"] = mouse("wheel_down")
    art["mouse_x"] = mouse("x")
    art["mouse_y"] = mouse("y")
    return art


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", required=True, help="glyph root, e.g. content/dev/ui/glyphs")
    args = parser.parse_args()
    root = Path(args.out)
    sets = {"kbm": kbm_set(), "xbox": gamepad_set("xbox"), "ps": gamepad_set("ps"), "nx": gamepad_set("nx")}
    for family, art in sets.items():
        folder = root / family
        folder.mkdir(parents=True, exist_ok=True)
        for name, canvas in art.items():
            (folder / f"{name}.png").write_bytes(canvas.png())
        print(f"{family}: {len(art)} glyphs")


if __name__ == "__main__":
    main()
