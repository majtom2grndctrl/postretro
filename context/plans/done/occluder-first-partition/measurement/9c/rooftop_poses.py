"""Rooftop pose set for stress-street (Task 9c, owner-requested).

Writes a `world_poses` pose file (`postretro-world-poses 1`) with eyes 1.5 m
above every building roof of `tools/stress_street.py`: the abutting row (A),
the broken row (B) and the free-standing plaza building. Two eye points per
roof footprint: its centre, and an edge point 1 m behind the street facade line
at mid-frontage. Each point gets 8 yaws (0..315 by 45) at pitch 0 and -15
(looking slightly down). Roof height is `building_height(storeys)`, the top of
the building's solid block, facade strips and room roof.

Map metres (x, y, z) -> engine metres (-y, z, -x), as `engine_pose` does;
engine yaw 0 faces -Z (map +X), counter-clockwise in plan.

usage: python3 rooftop_poses.py > stress-street.rooftop.poses
"""

import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, *[".."] * 6, "tools"))

import stress_street as st  # noqa: E402

EYE_M = 1.5
EDGE_INSET_M = 1.0
YAWS = tuple(range(0, 360, 45))
PITCHES = (0, -15)


def roofs():
    """(label, map x, map y, roof z) per eye point, in generator order."""
    out = []
    fronts, _ = st.frontages()
    for f in fronts:
        i, lo, hi = f["seg"], f["lo"], f["hi"]
        h = st.building_height(f["storeys"])
        back = (-st.BUILDING_D, 0.0) if f["side"] == "A" else (st.CANYON_W, st.CANYON_W + st.BUILDING_D)
        x0, y0, x1, y1 = st.span_rect(i, lo, hi, *back)
        name = f"{f['name']}.s{i}"
        out.append((f"{name}.c", (x0 + x1) / 2, (y0 + y1) / 2, h))
        edge_d = -EDGE_INSET_M if f["side"] == "A" else st.CANYON_W + EDGE_INSET_M
        ex, ey = st.route_point(i, (lo + hi) / 2, edge_d)
        out.append((f"{name}.e", ex, ey, h))
    (bx0, by0, bx1, by1), storeys, _ = st.FREE_BUILDING
    h = st.building_height(storeys)
    out.append(("free.c", (bx0 + bx1) / 2, (by0 + by1) / 2, h))
    # Free-standing building's east face looks up the street (+X).
    out.append(("free.e", bx1 - EDGE_INSET_M, (by0 + by1) / 2, h))
    return out


def main():
    points = roofs()
    lines = ["postretro-world-poses 1",
             f"# rooftop set: {len(points)} eye points ({len(points) // 2} roofs x centre/edge), "
             f"eye {EYE_M} m above roof, yaws {list(YAWS)}, pitches {list(PITCHES)}; "
             "source tools/stress_street.py; measurement/9c/rooftop_poses.py"]
    for label, x, y, z in points:
        ex, ey, ez = -y, z + EYE_M, -x
        for pitch in PITCHES:
            for yaw in YAWS:
                lines.append(f"pose roof:{label}@{yaw}/{pitch} {ex:g} {ey:g} {ez:g} {yaw} {pitch}")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
