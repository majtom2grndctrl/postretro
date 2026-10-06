"""Average Metal GPU Counters over each compose pass label's intervals.

usage: counters.py <trace> <out.json>
The trace must hold both Metal System Trace (pass intervals) and the Metal
GPU Counters instrument (50 us samples on the same clock). For every compose
label (A and [B] halves of a paired run) it averages Compute Shader Occupancy
and ALU utilization over the samples inside that label's intervals and sums
last-level-cache bytes per interval.
"""
import bisect, collections, json, subprocess, sys, tempfile
import xml.etree.ElementTree as E
from pathlib import Path

trace, out = sys.argv[1], sys.argv[2]
tmp = Path(tempfile.mkdtemp())


def export(schema):
    path = tmp / f"{schema}.xml"
    subprocess.run(["xcrun", "xctrace", "export", "--input", trace, "--xpath",
                    f'/trace-toc/run[@number="1"]/data/table[@schema="{schema}"]', "--output", str(path)],
                   check=True, capture_output=True)
    root = E.parse(path).getroot()
    path.unlink()
    ids = {e.attrib["id"]: e for e in root.iter() if "id" in e.attrib}
    return root, (lambda e: ids[e.attrib["ref"]] if "ref" in e.attrib else e)


info, res = export("gpu-counter-info")
names = {}
for row in info.iter("row"):
    cols = [res(c) for c in row]
    names[int(cols[1].text)] = cols[2].attrib.get("fmt")
values, res = export("gpu-counter-value")
samples = collections.defaultdict(list)  # counter -> [(t, value)]
for row in values.iter("row"):
    cols = [res(c) for c in row]
    samples[int(cols[1].text)].append((int(cols[0].text), float(cols[2].text)))
for s in samples.values():
    s.sort()
intervals, res = export("metal-gpu-intervals")
seen, spans = set(), collections.defaultdict(list)
for row in intervals.iter("row"):
    cols = list(row)
    if len(cols) < 17 or cols[10].tag == "sentinel" or "postretro (" not in res(cols[10]).attrib.get("fmt", ""):
        continue
    label = res(cols[6]).attrib.get("fmt", "").split("  (")[0].split(":")[0]
    if "SH Compose" not in label and "Animated Direct SH" not in label:
        continue
    key = (res(cols[15]).text, res(cols[16]).text, res(cols[0]).text)
    if key in seen:
        continue
    seen.add(key)
    start = int(res(cols[0]).text)
    spans[label].append((start, start + int(res(cols[1]).text)))
result = {}
for label, label_spans in spans.items():
    entry = {"intervals": len(label_spans)}
    for counter, name in names.items():
        series = samples.get(counter, [])
        times = [t for t, _ in series]
        inside = []
        for a, b in label_spans:
            lo, hi = bisect.bisect_left(times, a), bisect.bisect_right(times, b)
            inside.extend(v for _, v in series[lo:hi])
        entry[name] = round(sum(inside) / len(inside), 3) if inside else None
        entry[f"{name} samples"] = len(inside)
    result[label] = entry
Path(out).write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps(result, indent=2))
