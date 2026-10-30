"""Task 9g: tabulate the raw records into the README's tables (9c's collect.py, re-rooted).

before: sweep, bake-gap, rooftop and uncapped records re-measured with HEAD's harness in
target/occluder-9g/before/<map>/; its bake log, structure and regions from
target/occluder-baselines/<map>/; its id-51 and SH readers from 9c's side runs
(target/occluder-9c/before/<map>/, PRL readers that no harness change touches).
Configs: target/occluder-9g/<config>/<map>/. Prints markdown.

usage: python3 collect.py <config>...      (cap19 cap26)
"""

import os
import re
import sys

W = "/Users/dhiester/Projects/Personal/postretro"
T = f"{W}/target"
MAPS = ["occlusion-test", "stress-warren-hallway-inspection", "campaign-test",
        "stress-warren-showcase", "stress-street"]
SHORT = {"occlusion-test": "occlusion-test", "stress-warren-hallway-inspection": "hallway",
         "campaign-test": "campaign-test", "stress-warren-showcase": "arena",
         "stress-street": "street"}


def root(side, m):
    """Bake-side records: bake log, structure, regions."""
    return f"{T}/occluder-baselines/{m}" if side == "before" else f"{T}/occluder-9g/{side}/{m}"


def run_root(side, m):
    """Harness records measured in this task: sweep, bake gap, rooftop, uncapped."""
    return f"{T}/occluder-9g/{side}/{m}"


def reader_root(side, m):
    return f"{T}/occluder-9c/before/{m}" if side == "before" else f"{T}/occluder-9g/{side}/{m}"


def read(path):
    try:
        with open(path, errors="replace") as f:
            return f.read()
    except FileNotFoundError:
        return None


def sweep(text):
    if text is None:
        return None
    d = {}
    t = text[text.index("per pose"):]
    for line in t.splitlines()[1:25]:
        p = line.split()
        if len(p) == 4:
            try:
                d[p[0]] = (float(p[1]), float(p[2]), float(p[3]))
            except ValueError:
                pass
    m = re.search(r"paths \[(.*)\]", text)
    d["paths"] = m.group(1) if m else "?"
    d["trips"] = int(re.search(r"step-cap trips: (\d+)", text).group(1))

    def pair(label):
        m = re.search(label + r"[^:]*: (\d+), (\d+) with rays that did not miss before", text)
        return (int(m.group(1)), int(m.group(2))) if m else (None, None)
    d["miss_drawn"] = pair(r"poses with hits in unreached drawable cells")
    d["miss_culled"] = pair(r"poses with hits in cells entered across a culled opening")
    d["miss_faceless"] = pair(r"poses with hits in unreached faceless cells")
    d["verdict"] = (re.findall(r"verdict: (\w+)", text) or ["?"])[-1]
    return d


SEQ = r"^(random walk|far-point tour|pose tour)[^\n]*?: (\d+) frames in [\d.]+ s; deferred (\d+) blocks / (\d+) texels, left behind (\d+) blocks / (\d+) texels, (\d+) trip frames"


def bakegap(text):
    if text is None:
        return None
    rows = re.findall(SEQ, text, re.M)
    out = {}
    for name, frames, db, dt, lb, lt, trips in rows:
        out[name] = dict(frames=int(frames), def_blocks=int(db), def_texels=int(dt),
                         left_texels=int(lt), trips=int(trips))
    v = re.findall(r"verdict: (\w+)", text)
    out["verdict"] = v[-1] if v else "?"
    return out


def structure(text):
    if text is None:
        return None
    s = {}
    m = re.search(r"^cells (\d+) \(interior (\d+), solid (\d+), exterior (\d+)", text, re.M)
    s["cells"] = tuple(int(x) for x in m.groups())
    m = re.search(r"largest extent: p50 ([\d.]+) p95 ([\d.]+) max ([\d.]+)", text)
    s["largest"] = tuple(float(x) for x in m.groups())
    m = re.search(r"smallest extent: .*\[0.25-0.5 m\] (\d+)", text)
    s["thin"] = int(m.group(1))
    m = re.search(r"^portals (\d+) \(walkable, both cells interior: (\d+)", text, re.M)
    s["portals"] = (int(m.group(1)), int(m.group(2)))
    m = re.search(r"^faces (\d+), triangles (\d+)", text, re.M)
    s["faces"] = (int(m.group(1)), int(m.group(2)))
    m = re.search(r"^lightmap blocks (\d+) over (\d+) cells .*texel area (\d+)", text, re.M)
    s["lm"] = (int(m.group(1)), int(m.group(3)))
    m = re.search(r"^SH clusters (\d+) \(indivisible oversize (\d+)\)", text, re.M)
    s["shc"] = (int(m.group(1)), int(m.group(2)))
    m = re.search(r"^id 51: (\d+) camera cells, (\d+) entries", text, re.M)
    s["id51"] = int(m.group(2))
    return s


def bakelog(text):
    if text is None:
        return None
    b = {}
    m = re.search(r"Occluder classification: (\d+) brushes, (\d+) occlude alone; exempt alone: (\d+) under floor, "
                  r"(\d+) no open face, (\d+) short span, (\d+) low top; (\d+) clusters, (\d+) sections, (\d+) occlude "
                  r"\((\d+) member brushes\); constants: floor ([\d.]+) m, absolute length ([\d.]+) m", text)
    b["classify"] = tuple(m.groups()) if m else None
    m = re.search(r"Runtime cells: (\d+) occluder-tier leaves; (\d+) cells \((\d+) interior, (\d+) solid, (\d+) exterior\); "
                  r"(\d+) runtime portals; full tree (\d+) leaves, (\d+) portals(?:; cell-size cap ([\d.]+) m)?", text)
    b["runtime"] = tuple(m.groups()) if m else None
    seals = {}
    for mv, _ in re.findall(r"kinematic mover (\d+) seals generated portal (\d+)", text):
        seals[int(mv)] = seals.get(int(mv), 0) + 1
    b["seals"] = seals
    m = re.search(r"Door recognition: (\d+) of (\d+)", text)
    b["doors"] = (int(m.group(1)), int(m.group(2))) if m else None
    b["leak"] = bool(re.search(r"boundary leak|unsealed or have a leak", text))
    m = re.search(r"Exterior flood-fill: (\d+) exterior leaves, (\d+) interior empty leaves", text)
    b["flood"] = (int(m.group(1)), int(m.group(2))) if m else None
    m = re.search(r"Watertightness: (\d+) open edge", text)
    b["open_edges"] = int(m.group(1)) if m else 0
    extra = 0
    for a, c in re.findall(r"cutting face.*?into (\d+)x(\d+) sub-charts", text):
        extra += int(a) * int(c) - 1
    b["subcharts"] = extra
    return b


def mandatory(text):
    if text is None:
        return None
    m = re.search(r"mandatory texels p50 ([\d.]+) M p95 ([\d.]+) M max ([\d.]+) M", text)
    over = re.search(r"cells over 62.9 M: (\d+)", text)
    return (float(m.group(1)), float(m.group(2)), float(m.group(3)), int(over.group(1))) if m else None


def shreach(text):
    if text is None:
        return None
    m = re.search(r"SH mandatory clusters\s+median (\d+), p95 (\d+), max (\d+)", text)
    lm = re.search(r"lightmap pool layers\s+median (\d+), p95 (\d+), max (\d+)", text)
    return (tuple(int(x) for x in m.groups()) if m else None,
            tuple(int(x) for x in lm.groups()) if lm else None)


def probe_reach(text):
    if text is None:
        return None
    m = re.search(r"(\d+) clusters, (\d+) interior cells\n\s+probe reach into another cluster's cells: (\d+) clusters "
                  r"\((\d+) interior cells\), (\d+) cluster pairs, (\d+) probe refs", text)
    e = re.search(r"owner edges: patch (\d+) over (\d+) clusters", text)
    if not m:
        return None
    g = [int(x) for x in m.groups()]
    return dict(clusters=g[0], interior=g[1], reach_clusters=g[2], reach_cells=g[3], pairs=g[4], refs=g[5],
                patch_edges=int(e.group(1)) if e else None)


POOL = (r"pool: first (\d+) layers, peak (\d+) layers \((\d+) occupied; headroom cost (\d+) layers, "
        r"([\d.]+) MiB\), growths (\d+)")
LAYER_MIB = 14.0  # one pool layer (irradiance + direction + shadowmask), from the pool lines' MiB per layer


def pools(text):
    if text is None:
        return None
    return [dict(first=int(a), peak=int(b), occupied=int(c), headroom=int(d), headroom_mib=float(e), growths=int(f))
            for a, b, c, d, e, f in re.findall(POOL, text)]


def uncapped(text):
    if text is None:
        return None
    m = re.search(r"summary poses (\d+) mean ([\d.]+) p50 (\d+) p95 (\d+) p99 (\d+) max (\d+) worst (\S+) "
                  r"over5000 (\d+) total (\d+)", text)
    if not m:
        return None
    g = m.groups()
    return dict(n=int(g[0]), mean=float(g[1]), p50=int(g[2]), p95=int(g[3]), p99=int(g[4]), max=int(g[5]),
                worst=g[6], over=int(g[7]), total=int(g[8]))


def load(side, m):
    r = root(side, m)
    rr = run_root(side, m)
    sr = reader_root(side, m)
    return dict(
        sweep=sweep(read(f"{rr}/sweep.out")),
        bakegap=bakegap(read(f"{rr}/bakegap.out")),
        pools=pools(read(f"{rr}/bakegap.out")),
        unc_street=uncapped(read(f"{rr}/uncapped-street.txt")),
        unc_roof=uncapped(read(f"{rr}/uncapped-rooftop.txt")),
        structure=structure(read(f"{r}/structure.out")),
        bake=bakelog(read(f"{r}/bake.log")),
        mandatory=mandatory(read(f"{sr}/mandatory.out")),
        shreach=shreach(read(f"{sr}/shreach.out")),
        probe=probe_reach(read(f"{sr}/probe-reach.out")),
        rooftop=sweep(read(f"{rr}/rooftop-sweep.out")) if m == "stress-street" else None,
    )


def fmt(v):
    if v is None:
        return "–"
    if isinstance(v, float):
        return f"{v:,.1f}"
    if isinstance(v, int):
        return f"{v:,}"
    return str(v)


def table(title, sides, rows):
    print(f"\n### {title}\n")
    print("| Map | " + " | ".join(sides) + " |")
    print("|---|" + "---|" * len(sides))
    for label, vals in rows:
        print(f"| {label} | " + " | ".join(vals) + " |")


def main(configs):
    sides = ["before"] + configs
    names = ["before"] + configs
    data = {m: {s: load(s, m) for s in sides} for m in MAPS}

    def per_map(fn):
        return [(SHORT[m], [fn(data[m][s], s, m) for s in sides]) for m in MAPS]

    # M1
    def m1(d, s, m):
        w = d["sweep"]
        if w is None:
            return "–"
        i = 0 if s == "before" else 1  # before: all misses; configs: new against before
        dr, c, f = w["miss_drawn"][i], w["miss_culled"][i], w["miss_faceless"][i]
        return f"{w['trips']} trips, {dr} drawn / {c} culled / {f} faceless"
    table("M1: step-cap trips; poses with new drawn misses / reported culled / faceless", names, per_map(m1))

    # M2
    def m2(d, s, m):
        g = d["bakegap"]
        if not g or "pose tour" not in g:
            return "–"
        return " / ".join(fmt(g[k]["def_texels"]) for k in ("random walk", "far-point tour", "pose tour"))
    table("M2: deferred texels, random / far-point / pose tour", names, per_map(m2))

    def m2l(d, s, m):
        g = d["bakegap"]
        if not g or "pose tour" not in g:
            return "–"
        return " / ".join(f"{g[k]['left_texels'] / 1e6:,.1f} M" for k in ("random walk", "far-point tour", "pose tour"))
    table("M2 beside: left-behind texels, random / far / tour", names, per_map(m2l))

    def m2t(d, s, m):
        g = d["bakegap"]
        if not g or "pose tour" not in g:
            return "–"
        return " / ".join(fmt(g[k]["trips"]) for k in ("random walk", "far-point tour", "pose tour"))
    table("M2 beside: trip frames, random / far / tour", names, per_map(m2t))

    seqs = ("random walk", "far-point tour", "pose tour")

    def pk(d, s, m):
        p = d["pools"]
        if not p or len(p) != 3:
            return "–"
        return " / ".join(f"{x['peak']} ({(x['peak'] + 1) * LAYER_MIB:,.0f} MiB)" for x in p)
    table("M2 pool: peak layers (active MiB with the spare layer, 14.0 MiB per layer), random / far / tour", names, per_map(pk))

    def hr(d, s, m):
        p = d["pools"]
        if not p or len(p) != 3:
            return "–"
        return " / ".join(f"{x['headroom']} ({x['headroom_mib']:,.1f} MiB)" for x in p)
    table("M2 pool: headroom cost at peak, layers (MiB), random / far / tour", names, per_map(hr))

    def gr(d, s, m):
        p = d["pools"]
        if not p or len(p) != 3:
            return "–"
        return " / ".join(f"{x['first']}→{x['peak']}, {x['growths']}" for x in p)
    table("M2 pool: first → peak layers, growths, random / far / tour", names, per_map(gr))

    # M3 / M4
    def m3(d, s, m):
        b = d["bake"]
        if b is None:
            return "–"
        seals = ", ".join(f"m{k} {v}" for k, v in sorted(b["seals"].items())) or "none"
        return seals
    table("M3: seal lines per mover", names, per_map(m3))

    def m4(d, s, m):
        b = d["bake"]
        if b is None:
            return "–"
        fl = f"{b['flood'][0]} / {b['flood'][1]}" if b["flood"] else "?"
        return f"{'LEAK' if b['leak'] else 'none'}; ext/int leaves {fl}; open edges {b['open_edges']}"
    table("M4: leak; exterior / interior empty leaves (full tree); open edges", names, per_map(m4))

    # M5
    def cls(d, s, m):
        b = d["bake"]
        if not b or not b["classify"]:
            return "–"
        c = b["classify"]
        return f"{c[1]} alone; {c[8]} of {c[7]} sections ({c[9]} brushes); len {c[11]}"
    table("M5: occluders (occlude alone; sections occluding)", names, per_map(cls))

    def tier(d, s, m):
        b = d["bake"]
        if not b or not b["runtime"]:
            return "–"
        return fmt(int(b["runtime"][0]))
    table("M5: occluder-tier leaves", names, per_map(tier))

    def cells(d, s, m):
        st = d["structure"]
        return "–" if st is None else f"{st['cells'][0]:,} ({st['cells'][1]:,}/{st['cells'][2]:,}/{st['cells'][3]:,})"
    table("M5: cells (interior / solid / exterior)", names, per_map(cells))

    def portals(d, s, m):
        st = d["structure"]
        return "–" if st is None else fmt(st["portals"][1])
    table("M5: runtime walkable portals", names, per_map(portals))

    def faces(d, s, m):
        st, b = d["structure"], d["bake"]
        if st is None:
            return "–"
        sub = b["subcharts"] if b else 0
        return f"{st['faces'][0]:,} / {st['faces'][1]:,} ({sub} cut)"
    table("M5: PRL faces = lightmap charts / triangles (extra charts from atlas sub-chart cuts, already in the face count)", names, per_map(faces))

    def ext(d, s, m):
        st = d["structure"]
        return "–" if st is None else "{:.1f} / {:.1f} / {:.1f}".format(*st["largest"])
    table("M5: interior cell largest extent p50 / p95 / max (m)", names, per_map(ext))

    def thin(d, s, m):
        st = d["structure"]
        return "–" if st is None else fmt(st["thin"])
    table("M5: interior cells with smallest extent 0.25-0.5 m", names, per_map(thin))

    def shc(d, s, m):
        st = d["structure"]
        return "–" if st is None else f"{st['shc'][0]:,} ({st['shc'][1]})"
    table("M5: SH clusters (indivisible oversize)", names, per_map(shc))

    def mand(d, s, m):
        v = d["mandatory"]
        return "–" if v is None else f"{v[2]:.1f} M (p95 {v[1]:.1f}; {v[3]} over 62.9)"
    table("id-51 lightmap mandatory set at L 16 m, max per camera cell (texels)", names, per_map(mand))

    # M6
    for key, title in (("considered", "portals tested"), ("drawn", "drawn cells"),
                       ("tris_in_frustum", "tris in frustum"), ("lightmap_texels", "drawn lightmap texels")):
        def m6(d, s, m, key=key):
            w = d["sweep"]
            if w is None:
                return "–"
            mean, p95, _ = w[key]
            if key == "lightmap_texels":
                return f"{mean / 1e6:.2f} M / {p95 / 1e6:.2f} M"
            return f"{mean:,.1f} / {p95:,.0f}"
        table(f"M6: {title}, mean / p95 per pose", names, per_map(m6))

    def mx(d, s, m):
        w = d["sweep"]
        return "–" if w is None else f"{w['considered'][2]:,.0f} ({w['trips']} trips)"
    table("M6: portals tested, max per pose (a tripped pose stops at 5,000)", names, per_map(mx))

    # Guardrail: change of the per-pose means against before
    for key, title in (("considered", "portals tested"), ("tris_in_frustum", "tris in frustum"),
                       ("lightmap_texels", "drawn lightmap texels")):
        def g(d, s, m, key=key):
            w, b = d["sweep"], data[m]["before"]["sweep"]
            if w is None or b is None:
                return "–"
            return f"{(w[key][0] / b[key][0] - 1) * 100:+.0f}%"
        table(f"Guardrail: mean {title} per pose against before", names, per_map(g))

    # SH
    def shr(d, s, m):
        v = d["shreach"]
        return "–" if not v or not v[0] else "{} / {} / {}".format(*v[0])
    table("SH mandatory clusters per camera cell at L 16 m (owner closure included), median / p95 / max", names, per_map(shr))

    def pr(d, s, m):
        v = d["probe"]
        if v is None:
            return "–"
        return f"{v['pairs']:,} pairs, {v['refs']:,} refs; owner edges {v['patch_edges']:,}"
    table("SH probe reach into another cluster's cells (scratch reader)", names, per_map(pr))

    # M7
    def region(text):
        if text is None:
            return None
        t = text[text.index("=== region"):]
        cells = int(re.search(r"interior cells overlapping: (\d+)", t).group(1))
        m = re.search(r"walkable portals overlapping: (\d+), on (\d+) planes", t)
        planes = re.findall(r"(horizontal|vertical|oblique) (\w) = ([-\d.]+) m", t)
        return cells, int(m.group(1)), int(m.group(2)), planes

    def on_grid(v, base, step, n):
        k = (v - base) / step
        return abs(k - round(k)) < 2e-3 and 0 <= round(k) <= n

    def m7(d, s, m, which):
        r = region(read(f"{root(s, m)}/region-{which}.out"))
        if r is None:
            return "–"
        cells, portals, nplanes, planes = r
        out = f"{cells} cells, {portals} portals on {nplanes} planes"
        if which == "stairs":
            risers = [v for kind, ax, v in planes if ax == "z" and on_grid(float(v), 35.7632, 0.7112, 32)]
            treads = [v for kind, ax, v in planes if ax == "y" and on_grid(float(v), 1.6256, 0.3048, 32)]
            out += f"; riser planes {len(risers)}, tread planes {len(treads)}" + (f" ({', '.join(treads)})" if treads else "")
        return out
    for which in ("arena", "stairs"):
        print(f"\n### M7: {which} box\n")
        print("| " + " | ".join(names) + " |")
        print("|" + "---|" * len(sides))
        print("| " + " | ".join(m7(data["stress-warren-showcase"][s], s, "stress-warren-showcase", which) for s in sides) + " |")

    # Street trip overlap with the before build's trips
    def trips_of(path):
        t = read(path)
        if t is None:
            return None
        return {l.split()[1] for l in t.splitlines() if l.startswith("pose ") and " path=step_limit " in l}

    def rec(s, m, rooftop):
        if rooftop:
            return f"{run_root(s, m)}/rooftop.sweep"
        return f"{run_root(s, m)}/{m}.{'before' if s == 'before' else 'after'}.sweep"
    print("\n### Street step-cap trips: total / also tripped before / new\n")
    print("| Set | " + " | ".join(names) + " |")
    print("|---|" + "---|" * len(sides))
    for label, rooftop in (("street level (2,960 poses)", False), ("rooftop (608 poses)", True)):
        b = trips_of(rec("before", "stress-street", rooftop))
        cells_ = []
        for s in sides:
            t = trips_of(rec(s, "stress-street", rooftop))
            cells_.append("–" if t is None or b is None else f"{len(t)} / {len(t & b)} / {len(t - b)}")
        print(f"| {label} | " + " | ".join(cells_) + " |")

    # Rooftop
    print("\n### Rooftop set (stress-street, 608 poses)\n")
    print("| Row | " + " | ".join(names) + " |")
    print("|---|" + "---|" * len(sides))
    rt = [data["stress-street"][s]["rooftop"] for s in sides]

    def row(label, fn):
        print(f"| {label} | " + " | ".join("–" if r is None else fn(r) for r in rt) + " |")
    row("step-cap trips", lambda r: fmt(r["trips"]))
    row("new drawn-miss poses", lambda r: fmt(r["miss_drawn"][1]))
    row("culled / faceless (reported)", lambda r: f"{r['miss_culled'][1]} / {r['miss_faceless'][1]}")
    for key, label in (("considered", "portals tested mean / p95"), ("drawn", "drawn cells mean / p95"),
                       ("tris_in_frustum", "tris in frustum mean / p95")):
        row(label, lambda r, key=key: f"{r[key][0]:,.1f} / {r[key][1]:,.0f}")
    row("drawn lightmap texels mean / p95", lambda r: f"{r['lightmap_texels'][0] / 1e6:.2f} M / {r['lightmap_texels'][1] / 1e6:.2f} M")
    row("paths", lambda r: r["paths"])

    # Uncapped walk (scratch uncap9g: the unchanged walk with the step cap raised)
    for key, label in (("unc_street", "street level (2,960 poses)"), ("unc_roof", "rooftop (608 poses)")):
        print(f"\n### Uncapped portal tests per pose, {label}\n")
        print("| Row | " + " | ".join(names) + " |")
        print("|---|" + "---|" * len(sides))
        us = [data["stress-street"][s][key] for s in sides]

        def urow(lab, fn):
            print(f"| {lab} | " + " | ".join("–" if u is None else fn(u) for u in us) + " |")
        urow("mean", lambda u: f"{u['mean']:,.1f}")
        urow("p50 / p95 / p99", lambda u: f"{u['p50']:,} / {u['p95']:,} / {u['p99']:,}")
        urow("max (worst pose)", lambda u: f"{u['max']:,} (`{u['worst']}`)")
        urow("poses over 5,000", lambda u: fmt(u["over"]))
        urow("2× worst walk", lambda u: f"{2 * u['max']:,}")
        urow("total tests", lambda u: f"{u['total']:,}")


if __name__ == "__main__":
    main(sys.argv[1:])
