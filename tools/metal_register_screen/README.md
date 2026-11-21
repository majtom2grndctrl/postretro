# metal_register_screen

Reads how many registers AMD's Metal compiler gives the mover pipeline's
fragment shader. The mover shader sits near a register cliff on the RDNA1 Mac
target (`context/lib/rendering_pipeline.md` §7.3): a small WGSL edit can push
its VGPR count into a slower occupancy tier. Screen every edit to
`kinematic_brush.wgsl`, or to any snippet composed into it, before it lands.

## Requirements

- A Mac whose discrete GPU is AMD RDNA (the Radeon Pro 5300M target).
- Xcode command-line tools (`xcrun swiftc`) and Python 3.
- Run from a checkout; the script finds the repository from its own path.

## Usage

```bash
tools/metal_register_screen/screen_mover.sh <out dir>
```

It prints `fragment: sgprs=<n> vgprs=<n>` for the mover's `fs_main`. `<out dir>`
keeps the captured MSL (`vs_main.metal`, `fs_main.metal`), the serialized
archive (`pipeline.metallib`) and the capture's test log. Two captures diff
with `diff <a>/fs_main.metal <b>/fs_main.metal`.

The three steps, each runnable alone:

1. **Capture.** The ignored renderer test
   `kinematic_pipeline_msl_capture_for_the_register_screen` builds the real
   `KinematicBrushPass::new` on a Metal device and writes the MSL wgpu-hal logs
   for each entry point to `$POSTRETRO_REGISTER_SCREEN_OUT`. Because it calls
   the production constructor, it screens exactly what the engine compiles.
2. **Compile.** `screen.swift` (`xcrun swiftc -O screen.swift -o screen`, then
   `screen <out dir>`) compiles `vs_main`/`fs_main` for the discrete GPU through
   `MTLBinaryArchive` and serializes `pipeline.metallib`.
3. **Read.** `python3 -I regs.py <out dir>/pipeline.metallib` reads the AMD
   slice's `__TEXT,__fragment` section: after `air.packedFragmentFunction`, a
   u32 at +4 holds SGPRs and a u32 at +8 holds VGPRs. The format is
   reverse-engineered, not documented; re-derive the offsets if a result looks
   implausible. The vertex stage reports `not found`, which is expected.

## Reading a result

RDNA allocates VGPRs in granules, so a count that crosses into the next granule
can cost occupancy. Report a meaningful rise to the owner with a bisection of
the edit that caused it. A count is a screen, not a frame time: a change that
holds the count still needs a clock-pinned Metal System Trace (§12).

## Recorded counts (mover `fs_main`, Radeon Pro 5300M)

| Build | VGPRs | SGPRs |
|---|---|---|
| before specular-texel-sharpness | 126 | — |
| 74fcfd4b9 (texel-centre point, both styles) | 128 | 104 |
| specular-texel-sharpness: sharpness and tint with albedo read before the light loop, beside the tint accumulator | 224 | 101 |
| specular-texel-sharpness task 6, shipped: albedo read after the loop | 128 | 104 |
