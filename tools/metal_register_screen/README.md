# metal_register_screen

Reads how many registers AMD's Metal compiler gives each mover beauty
pipeline's fragment entry: `fs_pixel` and `fs_smooth`, one per specular style.
The mover shader sits near a register cliff on the RDNA1 Mac target
(`context/lib/rendering_pipeline.md` §7.3): a small WGSL edit can push an
entry's VGPR count into a slower occupancy tier. Screen both entries after
every edit to `kinematic_brush.wgsl`, or to any snippet composed into it,
before it lands. The tool also screens the fluid surface entries (see the
"Fluid surface entries" section).

## Requirements

- A Mac whose discrete GPU is AMD RDNA (the Radeon Pro 5300M target).
- Xcode command-line tools (`xcrun swiftc`) and Python 3.
- Run from a checkout; the script finds the repository from its own path.

## Usage

```bash
tools/metal_register_screen/screen_mover.sh <out dir> [fragment entry...]
```

It prints `<entry> fragment: sgprs=<n> vgprs=<n>` for each named fragment
entry, `fs_pixel` and `fs_smooth` by default. `<out dir>` keeps the captured
MSL (`<entry>.metal` for every entry wgpu compiled), one serialized archive per
screened entry (`<entry>.metallib`) and the capture's test log. Two captures
diff with `diff <a>/fs_smooth.metal <b>/fs_smooth.metal`.

The three steps, each runnable alone:

1. **Capture.** The ignored renderer test
   `kinematic_pipeline_msl_capture_for_the_register_screen` builds the real
   `KinematicBrushPass::new` on a Metal device and writes the MSL wgpu-hal logs
   for each entry point to `$POSTRETRO_REGISTER_SCREEN_OUT`. Because it calls
   the production constructor, it screens exactly what the engine compiles,
   and it fails unless both beauty entries were compiled.
2. **Compile.** `screen.swift` (`xcrun swiftc -O screen.swift -o screen`, then
   `screen <out dir> <fragment entry>`) compiles `vs_main` and that entry for
   the discrete GPU through `MTLBinaryArchive` and serializes
   `<fragment entry>.metallib`.
3. **Read.** `python3 -I regs.py <out dir>/<fragment entry>.metallib` reads the AMD
   slice's `__TEXT,__fragment` section: after `air.packedFragmentFunction`, a
   u32 at +4 holds SGPRs and a u32 at +8 holds VGPRs. The format is
   reverse-engineered, not documented; re-derive the offsets if a result looks
   implausible. The vertex stage reports `not found`, which is expected.

## Reading a result

RDNA allocates VGPRs in granules, so a count that crosses into the next granule
can cost occupancy. Under wave32 (granule 8 of 1024) up to 128 VGPRs holds 8
waves per SIMD; the next tier up halves that, and the next tier down needs 112
or fewer. Report a meaningful rise to the owner with a bisection of
the edit that caused it. A count is a screen, not a frame time: a change that
holds the count still needs a clock-pinned Metal System Trace (§12).

## Recorded counts (Radeon Pro 5300M)

Before mover-smooth-specular-pipeline: one `fs_main` pipeline drew both styles.

| Build | VGPRs | SGPRs |
|---|---|---|
| before specular-texel-sharpness | 126 | — |
| 74fcfd4b9 (texel-centre point, both styles) | 128 | 104 |
| specular-texel-sharpness: sharpness and tint with albedo read before the light loop, beside the tint accumulator | 224 | 101 |
| specular-texel-sharpness task 6, shipped: albedo read after the loop | 128 | 104 |

One beauty pipeline per style (mover-smooth-specular-pipeline):

| Build | Entry | VGPRs | SGPRs |
|---|---|---|---|
| shared per-record helpers, smooth one specular sum, albedo after the loop | `fs_pixel` | 127 | 104 |
| same | `fs_smooth` | 119 | 104 |
| same, `fs_smooth` reading albedo before the loop (screen only) | `fs_smooth` | 119 | 104 |

## Fluid surface entries

```bash
tools/metal_register_screen/screen_fluid.sh <out dir> [fragment entry...]
```

Screens the fluid pipelines' fragment entries: `fs_main_reflecting`,
`fs_main_reflecting_per_fragment` and `fs_main` by default. The capture test
`fluid_pipeline_msl_capture_for_the_register_screen` builds the production
fluid pipelines over their production layouts, then the reflecting entry once
more from the same source with the live-lighting sums blend replaced by the
per-fragment ratio it replaced, renamed `fs_main_reflecting_per_fragment`, so
one run screens before and after side by side. It also writes `vertex_layout`,
the fluid's three vertex streams, which `screen.swift` reads in place of the
mover's stream when present.

| Build | Entry | VGPRs | SGPRs |
|---|---|---|---|
| live lighting from per-probe sums | `fs_main_reflecting` | 72 | 98 |
| same source, per-fragment ratio (the per-fragment oracle) | `fs_main_reflecting_per_fragment` | 72 | 102 |
| same | `fs_main` | 55 | 100 |

## Glossy overlay entries

```bash
tools/metal_register_screen/screen_glossy.sh <out dir> [fragment entry...]
```

Screens the glossy world overlay's fragment entries, `fs_pixel` and
`fs_smooth` by default; the march-free `fs_pixel_flat` and `fs_smooth_flat`
are named explicitly. The capture test
`glossy_overlay_msl_capture_for_the_register_screen` builds every fragment
entry of the overlay's source over the production layout entries and
pipeline constructor, and writes `vertex_layout`, the overlay's world vertex
and probe streams. With `POSTRETRO_REGISTER_SCREEN_SOURCE` set to a WGSL file,
it screens that source instead, with no rebuild: an older overlay composed
from `git show <rev>:<shader>` in the order `GLOSSY_OVERLAY_SHADER_SOURCE`
names, or an ablation. Under wave32 the VGPR granule is 8 of 1024, so 64 or
fewer hold 16 waves per SIMD, 72 hold 14, 88 hold 11 and 96 hold 10.

| Build | Entry | VGPRs | SGPRs |
|---|---|---|---|
| 25a74b0d1, before Task 10: one `fs_main` | `fs_main` | 48 | 64 |
| 41ba36d8c, Task 10: the march, normal map, hemisphere axis, guard, texel point | `fs_pixel` | 96 | 80 |
| same | `fs_smooth` | 83 | 78 |
| per-form pipelines: `fs_pixel` / `fs_smooth` march and sample the normal map (materials that can march) | `fs_pixel` | 95 | 80 |
| same | `fs_smooth` | 84 | 78 |
| same, march-free (materials that never march, own normal map) | `fs_pixel_flat` | 62 | 64 |
| same | `fs_smooth_flat` | 61 | 64 |
| same, march-free with the neutral normal placeholder's one texel loaded | `fs_pixel_plain` | 63 | 64 |
| same | `fs_smooth_plain` | 61 | 64 |

The march-free entries fell from 71 / 67 to 62 / 61 when the mover guard's
mark moved from an early second `D_static` load to the composite's one load.
