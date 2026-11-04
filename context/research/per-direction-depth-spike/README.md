# Per-direction depth spike

Throwaway spike. Its results feed a future `/draft-session` for **Direction-aware probe visibility** (`context/plans/roadmap.md`, Lighting and GI). Nothing here merges. Only this README and its results outlive the branch.

Run on Windows, because the Mac's GPU is busy with an SH memory-cap experiment. Coordinator: the Mac session that ran the leak diagnosis.

## Why

The closet fixture showed the static-door reference glows, so the leak is the sampler's, not only the bake's. See `context/plans/drafts/mover-light-flow--probe-channel/plan.md` §Leak diagnosis result and `captures/verdict-sheet.png`.

`sh_corner_depth_visibility` (`sh_sample.wgsl`) applies Chebyshev to one depth mean and variance per probe, averaged over all 256 rays. A probe 0.09 m from a closed door has a long average, because most of its rays cross the bright room, so closet points 0.8 m behind the door pass the test. That cause comes from reading the code. Question 1 is also the first experiment to test it.

The proposed fix keeps the Chebyshev formula and stores the moments per direction, in a small octahedral tile per probe (Majercik et al. 2019).

## Read first

- `context/lib/index.md`, then its routing for rendering and the build pipeline.
- `content/dev/maps/mover-closet.README.md`, especially Probe lattice. Do not move the closet's north wall: y 760 is the worst measured phase, and other phases can read falsely dark.
- `context/plans/drafts/mover-light-flow--probe-channel/plan.md` §Leak diagnosis result.
- `sh_bake.rs` (`RAYS_PER_PROBE`, moment accumulation), `sh_sample.wgsl`, `crates/level-format/src/octahedral.rs`.

## Rules

- **IP (owner-reviewed, `context/research/gi-rtx-lite/ip-check.md`), even in throwaway code:** "Select diffuse probes by cell adjacency (own cell, then portal-adjacent) with continuous moment weights; store no per-probe zone or cell visibility mask and make no binary visible decision." Every probe keeps a continuous weight. No probe is excluded on a visibility test.
- Do not push to `main`, open a PR, or force-push. Fetch before pushing. If histories diverge, push a sibling branch.
- Hack freely: skip format versioning and streaming, and borrow a texture slot. The forward pass is at 16 of 16 sampled textures with cube arrays, so say which slot you borrowed. Label every hack in the results.
- Record GPU, driver, backend, commit and exact commands with each result. Separate measured from inferred.
- Pilot one run per configuration before any batch, and stop at the first invalid run.
- Do not call a visual result a pass. Report numbers and captures; the owner judges glow.

## Questions, in order

**0. Pilot: reproduce the baseline.** Build `mover-closet` and the static-door reference at this commit (`kit/gen_closet.py --static-door`, placed directly under `content/dev/maps`). Capture stations 2 and 3 from the fixture README with `POSTRETRO_SH_STREAMING=sync-proof`, feature `capture`, `force_full_resident_sh_compose`. `kit/` holds the Mac scripts: paths are hard-coded, and the README's stations are the authority over the views in `mk_scenes.py`. Expected on the Mac, post-tonemap sRGB 0–255:

| Build | Station 3 frame mean | Station 2 frame mean | Floor 0.04 m past the closet wall | 0.26 m | 0.48 m |
|---|---|---|---|---|---|
| Today, mover door | 6.60 | 6.83 | 23 | 19 | 17 |
| Reference, static door | 0.50 | 0.15 | 13 | 2 | 0 |

If the reference does not glow on Windows, **stop and report**. Small differences between machines are fine.

**1. Does per-direction depth clear the glow, and at what tile size?** Add per-direction `(E[d], E[d²])` tiles at 4×4, 6×6 and 8×8, and sample them in `sh_corner_depth_visibility` by the direction from probe to shaded point. Capture the static-door reference at each size, then:

- Compare with the table above, and with the outline on the door jambs.
- **Over-darkening check:** build the map with no door at all. Light must still flow through the open doorway, with no new dark band, compared with today's sampler. A fix that also blocks an open doorway fails.
- **Noise check:** 256 rays over an 8×8 tile is only 4 rays per texel. Rerun the smallest tile that works, and 8×8, at 1,024 rays. Report whether the result or any blotching changes.

The static door stands in for a mover's closed state, so this also bounds what closed-state tiles can do. Probe depth is baked with movers absent, so tiles alone will **not** fix a closed mover door. Do not try to solve that here.

**Stop rule:** if no tile size clears the glow, stop and report. That means the diagnosis or the approach is wrong, and the rest of the spike is moot.

**2. What does it cost per frame?** Measure forward-pass GPU time for today's sampler against each surviving tile size, on a heavy map if one is available here. Tiles filled with each probe's isotropic mean are fine for this, since cost does not depend on the values. Note that `POSTRETRO_GPU_TIMING=1` once lost the device on the full warren PRL on the GTX 1660 (`context/research/gi-rtx-lite/index.md`); pilot it first.

**3. How many probes need a tile?** On `stress-warren-hallway-inspection` (962,636 valid probes), count the probes whose per-direction result changes the outcome. Suggested test: for directions toward lattice neighbours, does per-direction visibility differ from isotropic visibility by more than 0.1 at the neighbour's distance? State the definition you used, and report the count in probes and, if you can, in dense SH slots.

## Context for pricing

From `spike/memory-measure-tour`, `context/plans/drafts/E20--memory-visibility/measurement/README.md` (Mac, 2026-11-03): SH pool 1,440 MiB, hallway VRAM 3,037 MiB p50 against a 2 GB target, 1,664 B per dense slot. Budget is tight, so any size that works counts, and smaller is better. An SH memory-cap experiment is running on the Mac and may change these figures.

## Results

Add a `## Windows` section here and push to this branch.
