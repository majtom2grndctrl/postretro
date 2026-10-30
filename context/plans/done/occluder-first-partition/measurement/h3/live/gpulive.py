"""GPU busy time per frame from a Metal System Trace attached to a live windowed run.

usage: gpulive.py <metal-gpu-intervals export .xml> <frame_ms> [top_n]
A live engine pipelines frames, so they overlap on the GPU and no interval delimits
them cleanly; render-pass encoder counts also go missing from parts of some traces
(rendering_pipeline.md §12). So the per-frame figure is normalised by time, not by
counted encoders:
  busy_frac = union of every postretro GPU interval (all channels, nested and coalesced
              rows counted once) / trace span (first interval start to last end)
  gpu_ms_per_frame = busy_frac * frame_ms, frame_ms = the run's CpuTiming total avg
Also reported: the same over graphics and compute channels only (paging and copy
engines excluded), and a counted cross-check (union / count of the once-per-frame
`Screen Effects Resolve Pass`). Per-pass rows are each pass's interval union per frame.
"""
import collections, math, re, sys
import xml.etree.ElementTree as ET


def union(iv):
    iv = sorted(iv)
    tot, cs, ce = 0, None, None
    for s, e in iv:
        if ce is None or s > ce:
            if ce is not None:
                tot += ce - cs
            cs, ce = s, e
        else:
            ce = max(ce, e)
    if ce is not None:
        tot += ce - cs
    return tot


def main():
    path = sys.argv[1]
    frame_ms = float(sys.argv[2]) if len(sys.argv) > 2 else math.nan
    top = int(sys.argv[3]) if len(sys.argv) > 3 else 14
    try:
        tree = ET.parse(path)
    except Exception as e:  # empty or truncated export
        print(f"gpu NO_TRACE ({type(e).__name__})")
        return
    ids = {e.attrib["id"]: e for e in tree.iter() if "id" in e.attrib}
    val = lambda e: ids[e.attrib["ref"]] if "ref" in e.attrib else e
    rows = []
    for r in tree.iter("row"):
        ch = [val(c) for c in r]
        proc = "".join(c.attrib.get("fmt", "") for c in ch if c.tag == "process")
        if "postretro" not in proc or len(ch) < 7:
            continue
        s = int(ch[0].text); d = int(ch[1].text); chan = ch[2].attrib.get("fmt", "")
        name = re.sub(r"\s+0x[0-9a-f]+$", "", ch[6].attrib.get("fmt", "").split(":")[0].strip())
        name = re.sub(r"\(\s*postretro \(\d+\)\s*\)", "(postretro)", name)
        rows.append((s, s + d, chan, name))
    if not rows:
        print("gpu NO_ROWS")
        return
    t0 = min(r[0] for r in rows); t1 = max(r[1] for r in rows); span = t1 - t0
    busy_all = union([(s, e) for s, e, c, n in rows])
    gc = [(s, e) for s, e, c, n in rows if not (c.startswith("SDMA") or c.startswith("Channel"))]
    busy_gc = union(gc)
    resolves = sum(1 for r in rows if r[3].startswith("Screen Effects Resolve"))
    frac_all = busy_all / span; frac_gc = busy_gc / span
    frames_by_time = span / 1e6 / frame_ms if frame_ms == frame_ms and frame_ms > 0 else math.nan
    print(f"gpu span_s={span / 1e9:.2f} frame_ms={frame_ms:.3f} frames_by_time={frames_by_time:.0f} resolve_passes={resolves} "
          f"busy_frac={frac_all:.3f} gpu_ms_per_frame={frac_all * frame_ms:.3f} "
          f"gfxcompute_frac={frac_gc:.3f} gfxcompute_ms_per_frame={frac_gc * frame_ms:.3f} "
          f"counted_ms_per_frame={(busy_all / 1e6 / resolves) if resolves else math.nan:.3f}")
    per = collections.defaultdict(list)
    for s, e, c, n in rows:
        per[(c, n)].append((s, e))
    nf = frames_by_time if frames_by_time == frames_by_time and frames_by_time > 0 else max(resolves, 1)
    for (c, n), iv in sorted(per.items(), key=lambda x: -union(x[1]))[:top]:
        print(f"  {c[:12]:12s} {n[:48]:48s} n={len(iv):6d} ms_per_frame={union(iv) / 1e6 / nf:7.3f}")


if __name__ == "__main__":
    main()
