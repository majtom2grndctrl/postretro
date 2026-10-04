"""Inclusive main-thread `sample` counts for the shadow-relevant CPU paths.

usage: profile.py <label> <frame_ms>
ms/frame = inclusive samples / main-thread samples × frame_ms (the clean,
unsampled median for that map and build). Statistical point estimate.
"""
import json, re, sys
from pathlib import Path

runs = Path(__file__).resolve().parent / "runs"
label, frame_ms = sys.argv[1], float(sys.argv[2])
text = (runs / f"{label}.sample.txt").read_text()
tree = text.split("Total number in stack")[0]
main = int(re.search(r"^\s+(\d+) Thread_\d+(?:: main|\s+DispatchQueue_\d+: com.apple.main-thread)",
                     tree, re.M)[1])
PATTERNS = {
    # wgpu-hal's Metal multi-draw expands to these per-slot calls.
    "hal_draw_indexed_indirect": r"DynCommandEncoder\d+draw_indexed_indirect(?!_)",
    "hal_draw_indexed": r"DynCommandEncoder\d+draw_indexed(?!_)",
    "core_encode_render_pass": r"wgpu_core\S*encode_render_pass",
    "core_command_encoder_finish": r"wgpu_core\S*command_encoder_finish",
    "shadow_reach_walk": r"ShadowReachIndex\S*5reach",
}
result = {"label": label, "main_thread_samples": main, "frame_ms": frame_ms, "paths": {}}
for key, pattern in PATTERNS.items():
    # A symbol can appear on several call paths; sum every distinct node.
    samples = sum(int(m) for m in re.findall(rf"^[^\n]*?\b(\d+) _R\S*{pattern}", tree, re.M))
    result["paths"][key] = {"samples": samples,
                            "ms_per_frame": round(samples / main * frame_ms, 4)}
print(json.dumps(result, indent=2))
