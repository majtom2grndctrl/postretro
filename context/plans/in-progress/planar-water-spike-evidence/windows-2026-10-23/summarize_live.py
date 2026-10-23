"""Reduce three tail CPU windows and one tail GPU window per live launch."""
import json
from pathlib import Path
import re
import statistics

ROOT = Path(__file__).resolve().parent / "live"
LABELS = ["total", "work", "visibility", "portal_walk", "mirror_walk", "render_record", "render_submit"]
records = []
for launch in json.loads((ROOT / "manifest.json").read_text()):
    key = launch["key"]
    log = (ROOT / (key + ".stderr.log")).read_text(encoding="utf-8")
    assert "SolidCellFallback" not in log and "Validation Error" not in log, key
    assert "render extents: surface=1600x900 scene=1600x900 divisor=1" in log, key
    cpu_lines = [line for line in log.splitlines() if "[CpuTiming] frames=120" in line][-3:]
    gpu_lines = [line for line in log.splitlines() if "[gpu-timing]" in line][-1:]
    assert len(cpu_lines) == 3 and len(gpu_lines) == 1, key
    stages = {}
    for label in LABELS:
        values = []
        for line in cpu_lines:
            match = re.search(r"(?:\s)" + label + r"=([0-9.]+)/([0-9.]+)ms", line)
            if match:
                values.append(float(match[1]))
        stages[label] = statistics.mean(values) if values else None
    planar = launch["planar"] == "1"
    assert (stages["mirror_walk"] is not None) == planar, key
    gpu = {}
    for label in ["fluid_reflection", "planar_reflection", "planar_forward", "forward", "fluid_surface"]:
        values = []
        for line in gpu_lines:
            match = re.search(r"(?:\s|\|)" + label + r" ([0-9.]+)ms(?: \((\d+)/120\))?", line)
            if match:
                assert match[2] in [None, "120"], (key, label, match[2])
                values.append(float(match[1]))
        gpu[label] = statistics.mean(values) if values else None
    assert (gpu["planar_reflection"] is not None) == planar, key
    assert (gpu["fluid_reflection"] is None) == planar, key
    records.append(dict(key=key, cpu_windows=3, cpu_frames=360, gpu_windows=1, gpu_readbacks=120, cpu_ms=stages, gpu_ms=gpu))
(ROOT / "summary.json").write_text(json.dumps(records, indent=2) + "\n", encoding="utf-8")
for row in records:
    print(row["key"], json.dumps(row["cpu_ms"]), json.dumps(row["gpu_ms"]))
