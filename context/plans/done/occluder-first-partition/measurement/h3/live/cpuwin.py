"""Summarise the [CpuTiming] windows of one live engine log.

usage: cpuwin.py <engine log> [skip_windows] [--field NAME] [--self-test]
Skips the first skip_windows windows (settling), then over the kept windows reports,
per stage, the mean of the window averages and the max of the window maxima
(rendering_pipeline.md §12: Time rows are label=avg/maxms, Count rows label=avg/max,
Marker rows label=frames, "(ran/frames)" on partial rows). Markers are summed.
Derived: us_per_test = portal_walk (ms) * 1000 / walk_considered, the live walk cost
per portal test. Prints one key=value line, or a single value with --field.
"""
import math, statistics, sys

KEYS = ["total", "work", "wait", "wait_acquire", "wait_present", "visibility", "portal_walk",
        "walk_considered", "walk_accepted", "portal_fallback", "render", "render_record",
        "render_submit", "rec_cull", "rec_forward", "lightmap_residency", "render_sh_drain"]
MARKERS = ["walk_step_limit"]


def parse_line(line):
    out = {}
    body = line.split("[CpuTiming]", 1)[1].split()
    for tok in body:
        name, eq, v = tok.partition("=")
        if not eq:
            continue
        ran = None
        if v.endswith(")") and "(" in v:
            v, _, frac = v[:-1].partition("(")
            ran = int(frac.split("/")[0])
        try:
            if v.endswith("ms"):
                a, m = v[:-2].split("/")
                out[name] = ("t", float(a), float(m), ran)
            elif "/" in v:
                a, m = v.split("/")
                out[name] = ("c", float(a), float(m), ran)
            else:
                out[name] = ("m", int(v), int(v), ran)
        except ValueError:
            continue
    return out


def summarise(path, skip):
    wins = [parse_line(l) for l in open(path, errors="replace") if "[CpuTiming]" in l]
    kept = wins[skip:]
    res = {"windows": len(kept), "windows_total": len(wins)}
    for k in KEYS:
        v = [w[k][1] for w in kept if k in w]
        res[k] = statistics.mean(v) if v else math.nan
        mx = [w[k][2] for w in kept if k in w]
        res[k + "_max"] = max(mx) if mx else math.nan
    for k in MARKERS:
        res[k] = sum(w[k][1] for w in kept if k in w)
    res["frames"] = sum(w["frames"][1] for w in kept if "frames" in w)
    pw, wc = res["portal_walk"], res["walk_considered"]
    res["us_per_test"] = pw * 1000.0 / wc if wc and not math.isnan(wc) and not math.isnan(pw) else math.nan
    return res


def fmt(v):
    return f"{v:.3f}" if isinstance(v, float) else str(v)


def self_test():
    line = ("[2026-10-29T10:00:00Z INFO  postretro::cpu_timing] [CpuTiming] frames=120 total=16.550/33.100ms "
            "work=8.330/11.100ms wait=8.220/20.000ms portal_walk=0.530/0.900ms walk_considered=1880.0/2100 "
            "walk_accepted=800.0/900 walk_step_limit=3 portal_fallback=0.010/0.020ms(3/120) render_submit=5.370/7.400ms")
    d = parse_line(line)
    assert d["total"] == ("t", 16.55, 33.1, None), d["total"]
    assert d["walk_considered"] == ("c", 1880.0, 2100.0, None)
    assert d["walk_step_limit"] == ("m", 3, 3, None)
    assert d["portal_fallback"] == ("t", 0.01, 0.02, 3)
    assert d["frames"] == ("m", 120, 120, None)
    print("cpuwin self-test OK")


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        self_test(); sys.exit(0)
    args = [a for a in sys.argv[1:]]
    field = None
    if "--field" in args:
        i = args.index("--field"); field = args[i + 1]; del args[i:i + 2]
    r = summarise(args[0], int(args[1]) if len(args) > 1 else 0)
    if field:
        v = r.get(field, math.nan)
        print("nan" if isinstance(v, float) and math.isnan(v) else fmt(v))
    else:
        print(" ".join(f"{k}={fmt(v)}" for k, v in r.items()))
