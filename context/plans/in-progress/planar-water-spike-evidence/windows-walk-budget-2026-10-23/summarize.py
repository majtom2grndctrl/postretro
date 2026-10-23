"""Reduce runs/results.json into tables.md and summary.json; re-verifies every log."""
from collections import defaultdict
import json
from pathlib import Path
import statistics

from run import reduce_log

ROOT = Path(__file__).resolve().parent
rows = json.loads((ROOT / "runs" / "results.json").read_text())
KEYS = ["effective_settings", "walk_steps", "extents", "cpu_windows", "cpu_frames", "gpu_windows", "gpu_readbacks", "cpu_ms", "gpu_ms", "mirror", "mirror_diagnostics", "lightmap"]
for row in rows:
    reduced = json.loads(json.dumps(reduce_log(ROOT / "runs" / (row["key"] + ".log"), row["settings"])))
    assert reduced == {k: row[k] for k in KEYS}, row["key"]
excluded = [row for row in rows if row["window"]["excluded"]]
groups = defaultdict(list)
for row in rows:
    if not row["window"]["excluded"]:
        groups[row["configuration"]].append(row)


def pair(group, fn):
    return " / ".join(fn(r) for r in group)


def ms(value):
    return "absent" if value is None else f"{value:.3f}"


tables = [
    "# Walk budget live results",
    "",
    "Release + dev-tools/capture, ordinary streaming, Vulkan, indirect validation off, VSync off. Entries are run 1 / run 2. CPU entries are five tail 120-frame window means; GPU entries are three tail 120-readback window means (logs round to 0.01 ms). CPU and GPU windows do not align. `mirror_walk` times only the mirror portal walk; the fallback per-cell cull is not in it but is inside `visibility`. `visibility` also contains the main walk (`portal_walk`).",
    "",
    "| Configuration | Frame, ms | Work, ms | Visibility, ms | Main walk, ms | Mirror walk, ms | Submit, ms | Reflection GPU, ms | planar_forward, ms | Mirror path | Considered | Drawable cells | Mirror-only |",
    "|---|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|---:|---:|",
]
summary = []
for name, group in groups.items():
    group.sort(key=lambda r: r["repeat"])
    on = group[0]["settings"]["on"]
    label = "planar_reflection" if on else "fluid_reflection"
    mirror = lambda r, k: "-" if r["mirror"] is None else str(r["mirror"][k])
    tables.append("| " + " | ".join([
        name,
        pair(group, lambda r: ms(r["cpu_ms"]["total"])),
        pair(group, lambda r: ms(r["cpu_ms"]["work"])),
        pair(group, lambda r: ms(r["cpu_ms"]["visibility"])),
        pair(group, lambda r: ms(r["cpu_ms"]["portal_walk"])),
        pair(group, lambda r: ms(r["cpu_ms"]["mirror_walk"])),
        pair(group, lambda r: ms(r["cpu_ms"]["render_submit"])),
        pair(group, lambda r: ms(r["gpu_ms"][label])),
        pair(group, lambda r: ms(r["gpu_ms"]["planar_forward"])),
        pair(group, lambda r: mirror(r, "path")),
        pair(group, lambda r: mirror(r, "considered")),
        pair(group, lambda r: mirror(r, "drawable")),
        pair(group, lambda r: mirror(r, "mirror_only")),
    ]) + " |")
    mean = lambda f: statistics.mean(f(r) for r in group) if all(f(r) is not None for r in group) else None
    summary.append(dict(
        configuration=name,
        settings=group[0]["settings"],
        repeats=len(group),
        frame_ms=mean(lambda r: r["cpu_ms"]["total"]),
        frame_ms_runs=[r["cpu_ms"]["total"] for r in group],
        work_cpu_ms=mean(lambda r: r["cpu_ms"]["work"]),
        visibility_ms=mean(lambda r: r["cpu_ms"]["visibility"]),
        main_walk_ms=mean(lambda r: r["cpu_ms"]["portal_walk"]),
        mirror_walk_ms=mean(lambda r: r["cpu_ms"]["mirror_walk"]),
        render_submit_ms=mean(lambda r: r["cpu_ms"]["render_submit"]),
        reflection_gpu_label=label,
        reflection_gpu_ms=mean(lambda r: r["gpu_ms"][label]),
        planar_forward_gpu_ms=mean(lambda r: r["gpu_ms"]["planar_forward"]),
        forward_gpu_ms=mean(lambda r: r["gpu_ms"]["forward"]),
        mirror=[r["mirror"] for r in group],
        lightmap=[r["lightmap"] for r in group],
        extents=group[0]["extents"],
        window=[r["window"] for r in group],
    ))
tables += ["", "Window probe per run (2 s samples): overlay = NVIDIA GeForce Overlay above the engine (noted confounder); user = user window above (excludes); foreground = engine had focus.", "", "| Run | Samples | Overlay | User above | Minimized | Foreground | Excluded |", "|---|---:|---:|---:|---:|---:|---|"]
for row in rows:
    w = row["window"]
    tables.append(f"| {row['key']} | {w['samples']} | {w['overlay_samples']} | {w['user_window_above_samples']} | {w['minimized_samples']} | {w['engine_foreground_samples']} | {w['excluded']} |")
(ROOT / "tables.md").write_text("\n".join(tables) + "\n", encoding="utf-8", newline="\n")
(ROOT / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8", newline="\n")
print(f"Summarized {len(rows) - len(excluded)} accepted runs across {len(summary)} configurations; excluded {[r['key'] for r in excluded]}.")
print("\n".join(tables[4:]))
