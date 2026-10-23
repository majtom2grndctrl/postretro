"""Verify retained live results and static illustration evidence."""
import hashlib
import json
from pathlib import Path
import struct

from run import reduce_log

root = Path(__file__).resolve().parent
rows = json.loads((root / "runs/results.json").read_text())
assert len(rows) == 16 and len({r["configuration"] for r in rows}) == 8
for row in rows:
    reduced = json.loads(json.dumps(reduce_log(root / "runs" / (row["key"] + ".log"), row["settings"])))
    assert reduced == {k: row[k] for k in ["effective_settings", "extents", "cpu_windows", "cpu_frames", "gpu_windows", "gpu_readbacks", "cpu_ms", "gpu_ms", "mirror"]}
    assert row["extents"] == ["1600", "900", "1600", "900", "1"]
images = {}
for directory in ["handoff-images", "handoff-repeat"]:
    for path in sorted((root / directory).glob("*.png")):
        data = path.read_bytes()
        assert data[:8] == b"\x89PNG\r\n\x1a\n" and struct.unpack(">II", data[16:24]) == (1600, 900)
        report = json.loads(path.with_suffix(".json").read_text())
        assert report["gpu_timing"]["availability"] == "available"
        windows = report["gpu_timing"]["windows"]
        assert len(windows) == 1 and windows[0]["readbacks"] == 120
        passes = {p["label"]: p for p in windows[0]["passes"]}
        assert all(p["malformed_readbacks"] == 0 for p in passes.values())
        planar = path.stem not in ["ssr-high", "probes-only"]
        assert passes["planar_reflection"]["sampled_readbacks"] == (120 if planar else 0)
        assert passes["fluid_reflection"]["sampled_readbacks"] == (120 if path.stem == "ssr-high" else 0)
        counters = report["lightmap_streaming"]["counters"]
        assert all(counters[k] == 0 for k in ["visible_misses_outside_baked_set", "visible_misses_not_resident", "failed_installs", "failed_reads"])
        images[path.relative_to(root).as_posix()] = hashlib.sha256(data).hexdigest()
assert len(images) == 8
for name in ["half-d15", "quarter-d15"]:
    assert images[f"handoff-images/{name}.png"] == images[f"handoff-repeat/{name}.png"]
assert images["handoff-images/quarter-d15.png"] == images["handoff-images/probes-only.png"]
sources = ["crates/renderer/src/render/planar_reflection/launch_settings.rs", "crates/renderer/src/render/planar_reflection/mod.rs", "crates/renderer/src/render/planar_reflection/tests.rs", "crates/renderer/src/render/renderer_init.rs"]
audit = dict(live_runs=16, cpu_frames=sum(r["cpu_frames"] for r in rows), gpu_readbacks=sum(r["gpu_readbacks"] for r in rows), static_captures=8, static_measured_frames=960, images_sha256=images, source_sha256={s: hashlib.sha256(Path(s).read_bytes()).hexdigest() for s in sources}, note="Quarter D15 is byte-identical to probes-only in this static pose; both D15 scales repeat byte-identically. No image quality acceptance inferred from performance.")
(root / "audit.json").write_text(json.dumps(audit, indent=2) + "\n")
print(json.dumps({k: v for k, v in audit.items() if k not in ["images_sha256", "source_sha256"]}, indent=2))
