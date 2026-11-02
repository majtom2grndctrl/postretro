"""Table of the traced 4K captures (trace_cap.sh -> trace/<tag>.frames.txt).

usage: parse_trace.py <trace dir>
Per pose and side: GPU busy per frame (median / p95; union of GPU intervals, see
gpuframes.py), frame span, the capture's completion median, and the per-frame mean
of the passes that move most (Textured, Depth Pre-Pass, Per-frame Upload Copies,
Page On paging, Kinematic Brush).
"""
import glob, os, re, sys
D = sys.argv[1]
PASSES = ["Textured Pass", "Depth Pre-Pass", "Per-frame Upload Copies", "Page On", "Kinematic Brush Pass"]
print("| Pose | Side | GPU busy ms median / p95 | span ms median | completion ms median | " + " | ".join(p.replace(' Pass', '') for p in PASSES) + " |")
print("|---|---|---|---|---|" + "---|" * len(PASSES))
def key(f):
    b = os.path.basename(f)[:-len(".r1.frames.txt")]
    pose, side = b.rsplit(".", 1)
    return (pose, ["before", "cap19", "cap26"].index(side))
for f in sorted(glob.glob(f"{D}/*.r1.frames.txt"), key=key):
    pose, side = os.path.basename(f)[:-len(".r1.frames.txt")].rsplit(".", 1)
    t = open(f).read()
    m = re.search(r"frames (\d+) busy median ([\d.]+) p95 ([\d.]+) ms \| span median ([\d.]+)", t)
    c = re.search(r"completion median ([\d.]+)", t)
    per = {p: next((float(x) for n, x in re.findall(r"^\s+(.+?)\s+([\d.]+) ms/frame", t, re.M) if n.startswith(p)), 0.0) for p in PASSES}
    print(f"| `{pose}` | {side} | {m.group(2)} / {m.group(3)} | {m.group(4)} | {c.group(1) if c else '– (cut at 90 s)'} | " + " | ".join(f"{per[p]:.2f}" for p in PASSES) + f" |")
