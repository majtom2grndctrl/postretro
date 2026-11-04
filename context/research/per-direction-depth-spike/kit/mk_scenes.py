#!/usr/bin/env python3
"""Write capture scenes: every view x {mover, static} (x mask variants)."""
import json, os, sys
S = 0.0254
ROOT = "/Users/dhiester/Projects/Personal/postretro"
HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "out")
os.makedirs(OUT, exist_ok=True)
os.makedirs(os.path.join(HERE, "scenes"), exist_ok=True)

def eng(x, y, z):
    return [-y * S, z * S, -x * S]

VIEWS = {
    # name: (map pos, yaw, pitch)
    "closet_to_door": ((320, 744, 64), 270, -6),
    "closet_floor_at_door": ((320, 610, 96), 270, -38),
    "closet_corner_west": ((184, 700, 64), 310, -8),
    "bright_to_door": ((320, 200, 48), 90, 0),
}
MAPS = {
    "mover": f"{ROOT}/content/dev/maps/mover-closet.prl",
    "static": f"{ROOT}/content/dev/maps/.mover-closet-static-ref.prl",
}
# LightTermMask bits, rendering_pipeline.md: 0 ambient, 1/2 static/animated
# indirect, 3/4 static/animated baked direct, 5 dynamic, 6 specular, 8 depth AO.
MASKS = {"all": None, "indirect": 1 << 1, "direct": 1 << 3}
for m, prl in MAPS.items():
    for v, (p, yaw, pitch) in VIEWS.items():
        for k, bits in MASKS.items():
            name = f"{m}__{v}__{k}"
            sc = {
                "map": prl,
                "camera": {"position": eng(*p), "yaw_deg": yaw, "pitch_deg": pitch, "fov_deg": 100.0},
                "resolution": [960, 540],
                "output": os.path.join(OUT, name + ".png"),
                "force_full_resident_sh_compose": True,
            }
            if bits is not None:
                sc["light_term_mask"] = bits
            json.dump(sc, open(os.path.join(HERE, "scenes", name + ".json"), "w"), indent=1)
print(len(os.listdir(os.path.join(HERE, "scenes"))))
