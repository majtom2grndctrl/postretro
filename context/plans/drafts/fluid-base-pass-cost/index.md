# fluid-base-pass-cost

Brief · compact · reads: `context/lib/rendering_pipeline.md` §7.3a, §7.4, §10, §12; `context/lib/development_guide.md` §1.4 · read at b0a5c0f58

## Problem
Developer-raised, from the cell-rate spike and an attribution experiment on the Radeon Pro 5300M, the Mac perf target (§10). Full-screen water at 4K costs 55 ms in the reflection-free fluid pass at e24 and 22 ms on the probes map, against a 24 ms and 20 ms world forward pass. The cause is the baked SH lookup, evaluated per fragment: it is 96% and 91% of the pass, and it runs twice per fragment, once for the indirect atlas and once for baked direct SH, which the world takes from its lightmap instead (`research.md` §Attribution). The lookup feeds a field that varies at probe spacing, so per-pixel evaluation buys nothing a sub-probe vertex grid does not. Done means the fluid's baked lighting is evaluated per vertex on an install-time grid, the pass costs about its non-SH floor, and water looks as it does today within the parity bounds below.

## Decisions
- **Baked terms per vertex.** Ambient floor, SH indirect and baked direct SH move from `fs_main` to `vs_main`, combined into one interpolated value under `LightTermMask` and the frame's scales. This is the billboard pass's shape (§7.4), which already reads group 3's SH per vertex. Diverges from §7.3a "Lighting. Per fragment": that wording exists so a dynamic light near a large face shows no faceting (`plans/done/E24--water-and-environment-volumes` acceptance), and the next bullet keeps that.
- **Per fragment stays per fragment.** Dynamic and promoted-static runtime lights, emissive, motion, depth fog and the reflection term are unchanged. Both shader variants take the per-vertex term, so they still match pixel for pixel where the reflecting gate is shut (§7.3a Two pipelines).
- **An install-time grid, fixed in world space.** Each surface face is cut by lattice planes aligned to the SH probe lattice, at most half a probe cell apart, in the face's plane. World-fixed planes cut an edge two faces share at the same points from both sides, so no new T-junction appears; the compiler's own T-junction vertices stay triangle corners (§7.3a Triangulation). Placement: render-cpu, at fluid install, GPU-free and unit-tested. No compiler, PRL or wire change.
- **Bounded.** Grid vertices are capped per level. A level past the cap coarsens its one spacing, for every face so shared edges still agree, and logs it, as install-time bounds do (`development_guide.md` §1.4). The bound is sized for an authored ocean, not for the test maps.
- **Viewer side per vertex.** The vertex flips the face normal toward the camera before the lookup, as the fragment does today. On a planar face every vertex agrees.
- **No new pass, binding or persistent GPU state.** Only the fluid vertex and index buffers grow. Reversible by reverting the shader and packing change.
- **All backends.** The lever removes fragment work on every backend; it is not a Metal-only switch. The GTX 1660 floor (§10) loses the same per-fragment sampler and gains vertex work bounded by the cap.
- **Non-goals:**
  - A tighter per-fragment SH sampler. Measured on the 5300M, reshapes that do less work ran up to 48% slower, and one equivalent reshape was faster at one pose and slower at the other (`research.md` §Code reshapes).
  - The reflection term: `reflection-probe-fluid-pass-performance` owns it.
  - The world forward pass's own per-fragment SH cost: unmeasured here (`research.md` §Wider finding).

## Acceptance
### Automated
- [ ] `fs_main` samples no SH atlas and no probe-depth texture; `vs_main` does. Runtime lights stay in `fs_main`.
- [ ] Grid packing, GPU-free:
  - triangle areas sum to the face's area, and every vertex lies in the face's plane and polygon;
  - every vertex's UV is the face's affine UV at its position;
  - two faces sharing an edge carry identical vertices along it, including faces of different volumes and the compiler's split faces;
  - every original vertex, collinear ones included, is still a triangle corner;
  - a face's vertices stay contiguous, and the drift and probe streams cover every one;
  - a face no lattice plane crosses packs as today;
  - a level whose faces would exceed the cap coarsens one spacing for every face, stays under the cap, and logs once.
- [ ] Reflection-free and reflecting variants match pixel for pixel wherever the reflecting gate is shut.
- [ ] Mask parity: each `LightTermMask` term off removes that term from the water, as today.
- [ ] A level without an SH volume draws water with no baked term and no error.
- [ ] Budget pins pass; both variants compile under the DX12 FXC test.
### Manual
- [ ] **5300M, near and probes 4K** (Metal kit, `research.md` §Method): reflection-free fluid pass at most 3.0 ms, whole-frame GPU time reported beside it. Today: 55.0 and 22.3 ms.
- [ ] **5300M at Auto (1536×960)**, near, probes, mid: measured and reported against today.
- [ ] **GTX 1660, Vulkan:** no measured view slower than today, and the reflecting-minus-reflection-free difference at e24 near 4K recorded before and after (`research.md` §Interaction). Measured and reported.
- [ ] **Parity:** against today's per-fragment image at near, probes and mid views, per-channel max and mean difference over water pixels, reported.
- [ ] **Owner look**, in engine over e24's pools and the probes map, steep and grazing: no visible grid lines, no light leak at pool walls, the water still matches baked light on adjacent walls, and a dynamic light near the water still lights it smoothly.

## Path
- **Seams:** `pack_fluid_surfaces` and `triangulate_convex_face` (render-cpu `fluid_surface`), `FluidSurfacePass::install`, `fluid_sample_sh` and `fs_main` (`fluid_surface.wgsl`), billboard `vs_main` for the vertex-side lookup.
- **SH lattice at install:** the SH volume installs with level geometry, before fluid install, so its origin and cell size can reach the packer.
- **Shape and rival:** per-vertex on a world-lattice grid; rival, texel-cell lighting in the reflection brief's cell pool (`research.md` §Rivals).
- **First slice:** grid packing with the shared-edge test, then `vertexsh` from the Metal kit on the gridded mesh, to price grid vertex work and read parity before the rest.
- **Measurement lever:** a capture-scene switch between per-vertex and per-fragment baked terms keeps the A/B in the kit.
- **Large files:** `fluid_surface.wgsl` is past 800 lines; split before extending.

## Open questions
- Accept the look change: baked water light interpolated across a sub-probe grid instead of evaluated per pixel — owner: Dan — **blocks build**
- Grid spacing within half a probe cell, and the per-level vertex cap — **delegated**: the executor picks them from parity and the 5300M cost and reports both.
- Parity bound for the owner's look to judge against — **delegated**: the executor reports measured numbers; no pass/fail threshold is set before the grid exists.
