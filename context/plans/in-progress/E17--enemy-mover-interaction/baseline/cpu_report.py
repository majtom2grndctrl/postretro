"""Median of each row 36 stage per run, in elapsed-time thirds, plus the combined sum.

usage: cpu_report.py <log> [<log> ...]
Each [CpuTiming] window reports stage=avg/peak ms. The first 2 windows are dropped.
"""
import pathlib
import re
import statistics
import sys

STAGES = ("sim_movers", "sim_steer_agents", "sim_steer_movers")


def windows(path):
    rows = []
    text = pathlib.Path(path).read_text(encoding="utf-8", errors="replace")
    for line in text.splitlines():
        if "[CpuTiming]" not in line or "frames=" not in line:
            continue
        rows.append(
            {k: float(v) for k, v in re.findall(r"(\w+)=([\d.]+)/[\d.]+ms", line)}
        )
    return rows[2:]


for path in sys.argv[1:]:
    rows = windows(path)
    name = pathlib.Path(path).stem
    if not rows:
        print(name, "no windows")
        continue
    third = max(len(rows) // 3, 1)
    parts = [rows[:third], rows[third : 2 * third], rows[2 * third :], rows]
    print(f"{name}: {len(rows)} windows")
    for label, part in zip(("early", "mid", "late", "all"), parts):
        values = [statistics.median(r.get(s, 0.0) for r in part) for s in STAGES]
        total = statistics.median(sum(r.get(s, 0.0) for s in STAGES) for r in part)
        sim = statistics.median(r.get("sim_tick", 0.0) for r in part)
        frame = statistics.median(r.get("total", 0.0) for r in part)
        cells = " ".join(f"{s}={v:.3f}" for s, v in zip(STAGES, values))
        print(f"  {label:5} {cells} combined={total:.3f} sim_tick={sim:.3f} total={frame:.3f}")
