"""ImageMagick AE/RMSE of each still against half-unlimited and probes-only."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("stills", type=Path)
args = parser.parse_args()
root = args.stills.resolve()
images = sorted(p for p in root.glob("*.png") if "-vs-" not in p.stem and not p.stem.startswith("diff"))
references = ["half-unlimited", "probes-only"]


def metric(kind, a, b):
    out = subprocess.run(["magick", "compare", "-metric", kind, str(a), str(b), "null:"], capture_output=True, text=True)
    assert out.returncode in (0, 1), out.stderr
    return out.stderr.strip()


results = {}
for image in images:
    data = image.read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n" and struct.unpack(">II", data[16:24]) == (1600, 900), image.name
    report = json.loads(image.with_suffix(".json").read_text())
    windows = report["gpu_timing"]["windows"]
    passes = {p["label"]: p for p in windows[0]["passes"]}
    counters = report["lightmap_streaming"]["counters"]
    entry = dict(
        sha256=hashlib.sha256(data).hexdigest(),
        gpu_readbacks=windows[0]["readbacks"],
        malformed=sum(p["malformed_readbacks"] for p in passes.values()),
        planar_readbacks=passes["planar_reflection"]["sampled_readbacks"],
        ssr_readbacks=passes["fluid_reflection"]["sampled_readbacks"],
        lightmap={k: counters[k] for k in ["visible_misses_outside_baked_set", "visible_misses_not_resident", "failed_installs", "failed_reads"]},
        compare={},
    )
    assert len(windows) == 1 and entry["gpu_readbacks"] == 120 and entry["malformed"] == 0, image.name
    assert all(v == 0 for v in entry["lightmap"].values()), image.name
    for ref in references:
        other = root / f"{ref}.png"
        if other == image or not other.exists():
            continue
        ae = metric("AE", image, other)
        rmse = metric("RMSE", image, other)
        entry["compare"][ref] = dict(ae=ae, ae_pixels=float(re.match(r"[0-9.e+]+", ae)[0]), rmse=rmse)
    results[image.stem] = entry
    print(image.stem, {k: (v["ae"], v["rmse"]) for k, v in entry["compare"].items()}, flush=True)
(root / "compare.json").write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8", newline="\n")
