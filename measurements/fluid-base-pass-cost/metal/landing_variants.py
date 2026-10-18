"""Write the landing A/B's reference modules into variants/.

usage: python3 measurements/fluid-base-pass-cost/metal/landing_variants.py [git ref]

`today.wgsl` is the fluid shader module as `ref` (default `main`) composed it,
per-fragment baked SH included: the engine built from this branch, with the
experiment hook applied, runs it through POSTRETRO_FLUID_WGSL_EXPERIMENT on the
same gridded mesh, so the A/B differs only in the shader. `today-flat.wgsl` is
that module drawing every water fragment opaque magenta: the water mask for
parity.py.
"""
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
# Order and separators of the reference's `fluid_surface.rs` SHADER_SOURCE.
PARTS = ("fluid_surface.wgsl", "sh_indirection.wgsl", "sh_sample.wgsl", "curve_eval.wgsl",
         "light_falloff.wgsl", "light_eval.wgsl")
FS_OPEN = "fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {\n"
FLAT = FS_OPEN + "    if uniforms.time > -1.0 { return vec4<f32>(1.0, 0.0, 1.0, 1.0); }\n"


def show(ref, name):
    return subprocess.run(["git", "show", f"{ref}:crates/renderer/src/shaders/{name}"], cwd=ROOT,
                          check=True, capture_output=True, text=True).stdout


def main():
    ref = sys.argv[1] if len(sys.argv) > 1 else "main"
    out = HERE / "variants"
    out.mkdir(exist_ok=True)
    today = "".join(show(ref, p) + "\n" for p in PARTS)
    assert today.count(FS_OPEN) == 1, "fs_main drifted"
    (out / "today.wgsl").write_text(today)
    (out / "today-flat.wgsl").write_text(today.replace(FS_OPEN, FLAT))
    print(f"wrote today.wgsl and today-flat.wgsl from {ref}")


if __name__ == "__main__":
    main()
