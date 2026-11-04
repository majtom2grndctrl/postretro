#!/usr/bin/env python3
"""Labelled contact sheet for the owner's glow verdict: sheet.py <capture dir> <out.html>."""
import base64, os, sys

D, OUT = sys.argv[1], sys.argv[2]
ROWS = [
    ("Today's sampler", "off-static", "off-nodoor"),
    ("Tiles, isotropic control", "static-iso", "nodoor-iso"),
    ("Tiles 4x4, 256 rays", "static-t4-r256", "nodoor-t4-r256"),
    ("Tiles 6x6, 256 rays", "static-t6-r256", "nodoor-t6-r256"),
    ("Tiles 8x8, 256 rays", "static-t8-r256", "nodoor-t8-r256"),
    ("Tiles 4x4, 1,024 rays", "static-t4-r1024", "nodoor-t4-r1024"),
    ("Tiles 8x8, 1,024 rays", "static-t8-r1024", "nodoor-t8-r1024"),
]


def uri(name):
    b = open(os.path.join(D, name + ".png"), "rb").read()
    return "data:image/png;base64," + base64.b64encode(b).decode()


def cell(name, gain):
    style = f' style="filter:brightness({gain})"' if gain != 1 else ""
    return f'<td><img src="{uri(name)}"{style} alt="{name}"></td>'


rows = []
for label, s, n in ROWS:
    rows.append(f"<tr><th>{label}</th>" + cell(s + "_threshold", 1) + cell(s + "_threshold", 8)
                + cell(s + "_door", 8) + cell(n + "_threshold", 1) + cell(n + "_door", 1) + "</tr>")
html = f"""<!doctype html><html><head><meta charset="utf-8"><title>Closet Glow Sheet</title>
<meta name="viewport" content="width=device-width, initial-scale=1"><style>
:root {{ --bg:#111; --fg:#ddd; }}
body {{ background:var(--bg); color:var(--fg); font:14px system-ui,sans-serif; margin:16px; }}
table {{ border-collapse:collapse; }} th {{ text-align:left; padding:4px 8px; font-weight:600; vertical-align:middle; }}
thead th {{ text-align:center; }} td {{ padding:2px; }} img {{ width:300px; display:block; }}
</style></head><body><h1>Closet glow: per-direction depth tiles</h1>
<p>Windows captures, GTX 1660 Super. The static door stands in for a closed door. Gain columns are brightened 8&times; to show what remains.</p>
<table><thead><tr><th></th><th>Static door, station 3</th><th>Static door, station 3, 8&times; gain</th>
<th>Static door, station 2, 8&times; gain</th><th>No door, station 3</th><th>No door, station 2</th></tr></thead><tbody>
{''.join(rows)}</tbody></table></body></html>"""
open(OUT, "w", encoding="utf-8").write(html)
print(OUT, len(html))
