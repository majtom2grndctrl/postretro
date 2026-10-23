"""Summarize matched release live runs, separate from earlier dev captures."""
from collections import defaultdict
import json
from pathlib import Path
import statistics

ROOT = Path(__file__).resolve().parent
rows = json.loads((ROOT / "runs" / "results.json").read_text())
groups = defaultdict(list)
for row in rows:
    groups[row["configuration"]].append(row)
tables = ["# Bounded live results", "", "Release + dev-tools/capture, ordinary streaming, Vulkan, indirect validation off, vsync launch seed off. CPU entries are five tail 120-frame window means per run; GPU entries are three tail 120-readback window means, from logs rounded to 0.01 ms. CPU and GPU windows do not align.", "", "| Configuration | Frame means, ms | Work CPU means, ms | Mirror walk means, ms | Submit mean, ms | Reflection GPU mean, ms |", "|---|---:|---:|---:|---:|---:|"]
summary = []
for name, group in groups.items():
    group.sort(key=lambda r: r["repeat"])
    def cpu_pair(label):
        return " / ".join(f"{r['cpu_ms'][label]:.3f}" if r["cpu_ms"][label] is not None else "absent" for r in group)
    def gpu_mean(label):
        values = [r["gpu_ms"][label] for r in group if r["gpu_ms"][label] is not None]
        return statistics.mean(values) if values else None
    label = "planar_reflection" if group[0]["settings"]["on"] else "fluid_reflection"
    reflection = gpu_mean(label)
    submit = statistics.mean(r["cpu_ms"]["render_submit"] for r in group)
    tables.append(f"| {name} | {cpu_pair('total')} | {cpu_pair('work')} | {cpu_pair('mirror_walk')} | {submit:.3f} | {reflection:.3f} |")
    summary.append(dict(configuration=name, settings=group[0]["settings"], repeats=len(group), frame_ms=statistics.mean(r["cpu_ms"]["total"] for r in group), work_cpu_ms=statistics.mean(r["cpu_ms"]["work"] for r in group), mirror_walk_ms=statistics.mean(r["cpu_ms"]["mirror_walk"] for r in group) if group[0]["settings"]["on"] else None, render_submit_ms=submit, reflection_gpu_ms=reflection, mirror=group[0]["mirror"], extents=group[0]["extents"]))
(ROOT / "tables.md").write_text("\n".join(tables) + "\n", encoding="utf-8")
(ROOT / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
print(f"Summarized {len(rows)} runs across {len(summary)} configurations.")
print("\n".join(tables[-len(summary):]))
