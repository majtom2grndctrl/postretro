"""Reduce saved capture reports; never interpret absent GPU passes as zero."""
import json
from collections import defaultdict
from pathlib import Path
import re
import statistics

ROOT = Path(__file__).resolve().parent


def gpu_pass(report, label):
    rows = [p for w in report["gpu_timing"].get("windows", []) for p in w["passes"] if p["label"] == label]
    count = sum(p["sampled_readbacks"] for p in rows)
    malformed = sum(p["malformed_readbacks"] for p in rows)
    return dict(ms=sum(p["average_ms"] * p["sampled_readbacks"] for p in rows if p["average_ms"] is not None) / count if count else None, samples=count, malformed=malformed)


def main():
    records = []
    tables = []
    for results_path in sorted(ROOT.glob("*/results.json")):
        suite = results_path.parent.name
        groups = defaultdict(list)
        for row in json.loads(results_path.read_text()):
            report = json.loads((results_path.parent / (row["key"] + ".json")).read_text())
            scene = json.loads((results_path.parent / (row["key"] + "-scene.json")).read_text())
            log = (results_path.parent / (row["key"] + ".log")).read_text()
            assert report["adapter"]["name"] == "NVIDIA GeForce GTX 1660 SUPER", row["key"]
            assert report["adapter"]["backend"] == ("Dx12" if suite == "dx12" else "Vulkan"), row["key"]
            assert report["gpu_timing"]["availability"] == "available", row["key"]
            assert sum(w["readbacks"] for w in report["gpu_timing"]["windows"]) == 240, row["key"]
            counters = report["lightmap_streaming"]["counters"]
            for field in ["visible_misses_outside_baked_set", "visible_misses_not_resident", "failed_installs", "failed_reads"]:
                assert counters[field] == 0, (row["key"], field)
            assert "SolidCellFallback" not in log and "Validation Error" not in log, row["key"]
            assert "indirect-call validation: on" in log, row["key"]
            passes = {label: gpu_pass(report, label) for label in ["planar_reflection", "planar_forward", "fluid_reflection", "forward", "fluid_surface", "bloom", "depth_prepass"]}
            assert not any(p["malformed_readbacks"] for w in report["gpu_timing"]["windows"] for p in w["passes"]), row["key"]
            planar = row["settings"]["on"] and row["quality"] != "off"
            if planar:
                assert passes["fluid_reflection"]["samples"] == 0 and passes["planar_reflection"]["samples"] == 240, row["key"]
                assert passes["planar_forward"]["samples"] == 240, row["key"]
            else:
                assert passes["planar_reflection"]["samples"] == 0, row["key"]
                if row["quality"] not in ["off", "low"]:
                    assert passes["fluid_reflection"]["samples"] == 240, row["key"]
                else:
                    assert passes["fluid_reflection"]["samples"] == 0, row["key"]
            mirror = re.findall(r"\[Planar reflection\] plane=([^ ]+) seeds=(\d+) visible=(\d+) reachable=(\d+) mirror-only=(\d+) path=([^ ]+) considered=(\d+) accepted=(\d+)", log)
            stages = defaultdict(list)
            for window in report["cpu_stages"].get("windows", []):
                for stage in window["stages"]:
                    stages[stage["label"]].append(stage["average"])
            item = dict(suite=suite, key=row["key"], configuration=row["configuration"], pose=row.get("pose"), repeat=row["repeat"], quality=row["quality"], settings=row["settings"], resolution=scene["resolution"], adapter=report["adapter"], completion_ms=report["cpu_completion"]["median_ms"], completion_p95_ms=report["cpu_completion"]["p95_ms"], passes=passes, cpu_average_ms={k: statistics.mean(v) for k, v in stages.items()}, mirror=mirror, lightmap_pool_bytes=counters["pool_bytes"])
            records.append(item)
            name = (row.get("pose", "") + " " + row["configuration"]).strip()
            groups[name].append(item)
        tables += [f"## {suite}", "", "| Configuration | Completion medians, ms | Reflection GPU mean, ms | Mirror forward, ms | Water composite, ms |", "|---|---:|---:|---:|---:|"]
        for name, group in groups.items():
            group.sort(key=lambda r: r["repeat"])
            def mean_pass(label):
                values = [r["passes"][label]["ms"] for r in group if r["passes"][label]["ms"] is not None]
                return f"{statistics.mean(values):.3f}" if values else "—"
            reflection = "planar_reflection" if group[0]["settings"]["on"] else "fluid_reflection"
            completion = " / ".join(f"{r['completion_ms']:.3f}" for r in group)
            tables.append(f"| {name} | {completion} | {mean_pass(reflection)} | {mean_pass('planar_forward')} | {mean_pass('fluid_surface')} |")
        tables.append("")
    (ROOT / "summary.json").write_text(json.dumps(records, indent=2) + "\n", encoding="utf-8")
    (ROOT / "tables.md").write_text("\n".join(tables) + "\n", encoding="utf-8")
    print(f"Validated {len(records)} captures: complete 240-readback reports, no malformed GPU samples, clean residency, expected reflection passes, validation on.")


if __name__ == "__main__":
    main()
