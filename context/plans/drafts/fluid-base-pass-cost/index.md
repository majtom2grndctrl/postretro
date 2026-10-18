# fluid-base-pass-cost

Brief · compact · reads: `context/lib/rendering_pipeline.md` §7.3a, §7.4, §10, §12; `context/lib/build_pipeline.md` §Compiler pipeline (Fluid surfaces); `context/lib/development_guide.md` §1.4 · read at 2f1f96297

## Problem
Developer-raised, from the cell-rate spike and an attribution experiment on the Radeon Pro 5300M, the Mac perf target (§10). Full-screen water at 4K costs 55 ms in the reflection-free fluid pass at e24 and 22 ms on the probes map, against a 24 ms and 20 ms world forward pass. The cause is the baked SH lookup, evaluated per fragment: it is 96% and 91% of the pass, and it runs once per baked atlas, indirect and direct SH, where the world takes baked direct from its lightmap (`research.md` §Attribution). The lookup feeds a field that varies at probe spacing, so per-pixel evaluation buys nothing a sub-probe vertex grid does not. Done means the fluid's baked lighting is evaluated per vertex on an install-time grid, the pass costs about its non-SH floor, and the owner accepts the look against the parity numbers below.

## Decisions
- **Baked terms per vertex.** Ambient floor, SH indirect and baked direct SH move from the fragment stage to `vs_main` and reach the fragment interpolated, still under `LightTermMask` and the frame's scales. The billboard pass already samples group 3's SH in `vs_main` (§7.4), though per sprite, not interpolated across a mesh. Diverges from §7.3a, which keeps every term per fragment against per-vertex faceting: E24 asks that of dynamic light (`plans/done/E24--water-and-environment-volumes`), kept per fragment below, and the sub-probe grid holds it off for baked terms; §7.3a narrows at landing.
- **Per fragment stays per fragment.** Dynamic and promoted-static runtime lights, emissive, motion, the depth fog's path and opacity, and the reflection term are unchanged; the fog layer's colour takes the interpolated baked term as it takes today's. Both shader variants take the per-vertex term, so they still match pixel for pixel where the reflecting gate is shut (§7.3a Two pipelines).
- **An install-time grid, fixed in world space.** Each surface face is cut, in its plane, by planes on the base SH lattice and its subdivisions. Fixed planes give two faces the same crossings along an edge they share, so the grid adds no T-junction. The T-junctions the compiler leaves open, burial's and those between volumes (`build_pipeline.md` §Fluid surfaces), stay as they are today.
- **Placed in render-cpu, at fluid install.** GPU-free and unit-tested. Not in prl-build: density stays tunable without a rebake, and there is no compiler, PRL or wire change (`research.md` §Rivals).
- **Bounded.** Grid vertices are capped per level, sized for an authored ocean, not for the test maps. A level past the cap takes the finest spacing that fits, for every face so shared edges still agree, chooses it before cutting any face, and warns once (`development_guide.md` §1.4, bound the work).
- **Viewer side per vertex.** The vertex lookup takes the camera-facing side of the face normal, as the fragment does today; a planar face's vertices all agree.
- **No new pass, binding or persistent GPU state.** Only the fluid mesh and its per-vertex drift and probe streams grow, with the grid's vertex count.
- **All backends.** The lever removes fragment work on every backend; it is not a Metal-only switch, and the GTX 1660 floor (§10) is measured beside the 5300M.
- **Non-goals:**
  - A tighter per-fragment SH sampler. Measured on the 5300M, reshapes that do less work ran up to 48% slower, and one equivalent reshape was faster at one pose and slower at the other (`research.md` §Code reshapes).
  - The reflection term: `reflection-probe-fluid-pass-performance` owns it (`research.md` §Interaction).
  - The world forward pass's own per-fragment SH cost: unmeasured here (`research.md` §Wider finding).
  - Baked light that answers a per-pixel normal. Today's wave moves UVs, not normals; normal-mapped water re-opens it (`research.md` §Foreclosure).

## Acceptance
### Automated
- [ ] Neither fragment entry samples an SH atlas, the probe-depth texture or the SH grid, under any gate; `vs_main` samples each. Runtime lights stay in both fragment entries, and the reflecting entry keeps the shared sampler for its probe reads. Today's pins that lighting is per fragment now assert the opposite.
- [ ] The fluid layout keeps today's groups, entries and stage visibilities, and today's vertex stride.
- [ ] Grid packing, GPU-free:
  - triangle areas sum to the face's area, no triangle has zero area, and every vertex lies in the face's plane and polygon;
  - every vertex the grid adds lies on a base-lattice plane at the spacing in force, and no sub-polygon spans more than that spacing on any lattice axis (P8);
  - every vertex's UV is the face's affine UV at its position;
  - two faces sharing an edge carry identical vertices along it, in either winding and after coarsening: faces of different volumes and the compiler's split faces, a top and a side, a sloped face, an edge lying in a lattice plane (it gains no vertex from that plane), and crossings just inside and just outside the collinear tolerance of an original vertex (P9–P12);
  - an edge the compiler left with a T-junction keeps it and gains no other, and its two sides' crossings agree within the collinear tolerance (P13);
  - every original vertex, collinear ones included, is still a triangle corner, and none is duplicated where a plane passes through it (P10);
  - a face's vertices stay contiguous, and the drift and probe streams cover every one, also after a drift re-resolve (P15);
  - a face no lattice plane crosses packs as today;
  - cap: a level at or under the cap keeps the finest spacing and raises no cap warning; one past it takes the finest spacing that fits for every face, those packed before the overflow included, keeps shared edges identical, ends at or under the cap and warns once however many steps it took; the spacing does not depend on face or volume order; a face whose finest grid would be far past the cap packs at or under it without building the finer mesh; a level whose ungridded vertices alone exceed the cap packs as today and warns once (P5–P7, P17, P18).
- [ ] The grid takes the lattice the SH sampler uses: the PRL section's, or a streamed level's manifest base, never the placeholder lattice an absent volume installs (P3).
- [ ] A level without a usable SH volume (none, one the device rejects, or one with a zero or non-finite cell size) packs as today and draws water with the ambient floor alone and no error (P4, P20); a level without fluid builds no grid and logs nothing (P14).
- [ ] Fluid install follows geometry install on the level and capture paths; a level change packs the second level on its own lattice, or none, with its own spacing and cap warning (P1, P2, P19).
- [ ] GPU frame tests, on a scene with a nonzero baked term and a face the lattice crosses; a run that skips for want of an adapter is not a pass:
  - reflection-free and reflecting variants match pixel for pixel wherever the reflecting gate is shut;
  - on a uniform baked field, water matches the CPU expectation of today's formula within 1/255, at the default and at changed frame scales, with each `LightTermMask` term on and off, in the water and its fog layer;
  - on a field that differs across the water plane, water takes its viewer side's baked term from above, from below and on the frame the camera crosses, on both variants (P16).
- [ ] Budget pins pass unedited, per stage and per group, the vertex stage included.
- [ ] The diff touches no level-compiler, level-format or wire code.
### Manual
- [ ] **5300M, near and probes 4K** (Metal kit poses `near` and `test`, `research.md` §Method): reflection-free fluid pass at most 3.0 ms, whole-frame GPU time reported beside it. Today: 55.0 and 22.3 ms.
- [ ] **5300M at Auto (1536×960)**, near, probes, mid: measured and reported against today.
- [ ] **Windows, DX12:** both variants compile under the on-demand FXC test.
- [ ] **GTX 1660, Vulkan:** no measured view slower than today beyond run-to-run spread, and the reflecting-minus-reflection-free difference at e24 near 4K recorded before and after (`research.md` §Interaction). Measured and reported.
- [ ] **Parity:** against today's per-fragment image at near, probes, mid and one view from under e24's surface, per-channel max and mean difference over water pixels, and again over water pixels within one grid spacing of a pool wall, reported.
- [ ] **Owner look**, in engine over e24's pools and the probes map, steep, grazing and from below: no visible grid lines, no light leak at pool walls, the water still matches baked light on adjacent walls, and a dynamic light near the water still lights it smoothly.

## Path
- **Seams:** `pack_fluid_surfaces` and `triangulate_convex_face` (render-cpu `fluid_surface`), `install_fluid_surfaces` and `FluidSurfacePass::install`, `fluid_sample_sh` and `fluid_shade` (`fluid_surface.wgsl`), billboard `vs_main` for the vertex-side lookup.
- **SH lattice at install:** the SH volume installs with level geometry, before fluid install. `ShVolumeResources` keeps neither origin nor cell size on the CPU, and a streamed level's lattice lives in its manifest base, so both values are plumbed to the packer from level data.
- **Shape and rival:** per-vertex on a world-lattice grid; rival, texel-cell lighting in the reflection brief's cell pool (`research.md` §Rivals).
- **First slice:** grid packing with the shared-edge test, then `vertexsh` from the Metal kit on the gridded mesh, to price grid vertex work and read parity before the rest.
- **Measurement:** the A/B runs through the kit's shader-override hook against today's module. No per-fragment switch ships: it would keep the sampler compiled into the measured shader and move its register allocation (`research.md` §Code reshapes).
- **Large files:** `fluid_surface.wgsl` is past 800 lines; split before extending.

## Open questions
- Accept the look change in principle: baked water light interpolated across a sub-probe grid instead of evaluated per pixel; the owner look on the first slice's parity numbers confirms it — owner: Dan — **blocks build**
- Grid spacing, starting at half a probe cell, and the per-level vertex cap — **delegated**: the executor picks them from parity and the 5300M cost and reports both.
- Parity bound for the owner's look to judge against — **delegated**: the executor reports measured numbers; no pass/fail threshold is set before the grid exists.
