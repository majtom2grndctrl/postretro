"""Pick the H3 guardrail poses: where tris in frustum rose most against before.

Reads 9g's sweep records (target/occluder-9g/<side>/<map>/*.sweep) and prints the
poses ranked by the larger of the cap19 and cap26 rise in tris_in_frustum, with
their texels, so the 3-4 costliest can be timed. usage: pick_tris.py <map> [n]
"""
import re, sys
G = "/Users/dhiester/Projects/Personal/postretro/target/occluder-9g"
m = sys.argv[1]; n = int(sys.argv[2]) if len(sys.argv) > 2 else 10

def load(path):
    out = {}
    for line in open(path):
        if not line.startswith("pose "):
            continue
        f = line.split()
        kv = dict(x.split("=", 1) for x in f[7:] if "=" in x)
        out[f[1]] = (f[2:7], kv)
    return out

b = load(f"{G}/before/{m}/{m}.before.sweep")
c19 = load(f"{G}/cap19/{m}/{m}.after.sweep")
c26 = load(f"{G}/cap26/{m}/{m}.after.sweep")
rows = []
for k, (pose, kv) in b.items():
    if k not in c19 or k not in c26:
        continue
    t0 = int(kv["tris_in_frustum"]); t19 = int(c19[k][1]["tris_in_frustum"]); t26 = int(c26[k][1]["tris_in_frustum"])
    x0 = int(kv["lightmap_texels"]); x19 = int(c19[k][1]["lightmap_texels"]); x26 = int(c26[k][1]["lightmap_texels"])
    rows.append((max(t19, t26) - t0, k, pose, t0, t19, t26, x0, x19, x26))
rows.sort(reverse=True)
print("# rise label x y z yaw pitch | tris before cap19 cap26 | texels(M) before cap19 cap26")
for r in rows[:n]:
    print(f"{r[0]} {r[1]} {' '.join(r[2])} | {r[3]} {r[4]} {r[5]} | {r[6]/1e6:.1f} {r[7]/1e6:.1f} {r[8]/1e6:.1f}")
