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

## Windows

All four questions are measured. The owner has not yet judged the captures, so none of this is a pass. Findings are at the end of this section.

**Setup.** NVIDIA GeForce GTX 1660 SUPER, driver 621.01, Vulkan, Windows 11. Base 52938ca84. `dev` profile, feature `capture`. Captures use `POSTRETRO_SH_STREAMING=sync-proof` and `force_full_resident_sh_compose`, 960×540, the views in `kit/final.py`. Scripts are in `kit/win/`, run from the worktree root:

```bash
python3 context/research/per-direction-depth-spike/kit/win/closet.py gen
bash context/research/per-direction-depth-spike/kit/win/q1.sh <scratch dir> tiles
bash context/research/per-direction-depth-spike/kit/win/q1.sh <scratch dir> run
```

**Instrument.** Frame mean is Rec. 709 luma, which reproduces the Mac's four PNGs exactly. Capture FOV is horizontal. The floor rows are not the Mac's formula: this script reads 20.1 / 16.1 / 13.9 and 11.0 / 1.8 / 0.0 on the Mac's PNGs, against 23 / 19 / 17 and 13 / 2 / 0 in the table above. All floor rows below use this script, on both machines' images.

### Q0: the reference glows here

| Build | Station 3 mean | Station 2 mean | Floor 0.04 m | 0.26 m | 0.48 m |
|---|---|---|---|---|---|
| Mac PNGs, mover door | 6.60 | 6.83 | 20.1 | 16.1 | 13.9 |
| Windows, mover door | 6.66 | 6.87 | 20.1 | 16.0 | 13.9 |
| Mac PNGs, static door | 0.50 | 0.15 | 11.0 | 1.8 | 0.0 |
| Windows, static door | 0.50 | 0.15 | 11.0 | 1.8 | 0.0 |

### Q1: per-direction depth

The sheet for the owner is `captures-win/verdict-sheet.png`: the static reference at today / 4×4 / 6×6 / 8×8, stations 2 and 3, at true brightness and 8× gain. Individual captures are beside it.

Static-door reference, 256 rays:

| Sampler | Station 3 mean | Station 2 mean | Floor 0.04 m | 0.26 m | 0.48 m |
|---|---|---|---|---|---|
| Today | 0.50 | 0.15 | 11.0 | 1.8 | 0.0 |
| 4×4 | 0.13 | 0.03 | 1.3 | 0.0 | 0.0 |
| 6×6 | 0.05 | 0.01 | 0.7 | 0.0 | 0.0 |
| 8×8 | 0.04 | 0.01 | 0.6 | 0.0 | 0.0 |

Measured controls:

- **Tiles off.** The patched binary is byte-identical to the baseline on all six static-door and no-door captures.
- **Isotropic control.** Every texel holds the probe's own single mean, read through the same tile path. The glow returns exactly: 0.50 / 0.15, floor 11.0 / 1.8 / 0.0. So direction is what removes the leak, not the tile path. This is the first experimental support for the cause the README inferred from reading the code.
- **Open doorway.** With no door, frame means are 29.86 / 15.03–15.04 at every size, against 29.86 / 15.04 today, and the floor is unchanged at 20.0 / 15.8 / 13.7. The worst view is 8×8 at station 2: 445 of 518,400 pixels move by more than 2 levels, with a maximum of −8.7. No dark band is visible in the captures.
- **Noise.** At 1,024 rays, frame means and floors move by at most 0.01. At most 345 pixels move by more than 2 levels (4×4, station 3). At 256 rays an 8×8 tile has at least 1 ray per texel and no empty texel.

**What remains.** At 8× gain a thin outline stays on the door edges and the floor strip at every size, thinnest at 6×6 and 8×8.

Diagnostic at 6×6, for cause only, not a proposed fix:

| Variant | Station 3 mean | Station 2 mean | Floor 0.04 m |
|---|---|---|---|
| Shipping floor 0.03, bias 5% | 0.05 | 0.01 | 0.7 |
| Floor 0.003 | 0.03 | 0.01 | 0.2 |
| Bias 0 | 0.05 | 0.01 | 0.7 |
| Floor 0.003 and bias 0 | 0.03 | 0.01 | 0.1 |

Most of the floor residual is `SH_DEPTH_MIN_VISIBILITY`. The bias alone changes nothing. The frame mean stops at 0.03, so something else holds the rest; its source is not isolated. With both changes the open doorway reads the same frame means as the shipping constants.

### Q2: forward-pass cost

Hallway PRL from 2026-10-31, 1920×1080, one camera position at three yaws, 240 warmup and 600 sample frames, `POSTRETRO_GPU_TIMING=1`, feature `capture,dev-tools`. `kit/win/q2.py` interleaves configurations. Base is an unpatched build of 52938ca84. No run lost the device.

`forward` in ms, mean of two runs, tiles as synthetic noise with one tile per SH slot:

| View | Base | Patched, tiles off | 4×4 | 6×6 | 8×8 |
|---|---|---|---|---|---|
| a | 2.01 | 2.01 | 2.35 | 2.34 | 2.36 |
| b | 3.04 | 3.06 | 3.19 | 3.20 | 3.24 |
| c | 2.55 | 2.59 | 2.79 | 2.77 | 2.74 |

- **Cost.** Tiles add 0.16–0.35 ms, or 5–17%. Tile size makes no measurable difference. Run-to-run spread reaches about 0.1 ms.
- **Whole frame.** CPU completion stays at about 9–10 ms with no clear change.
- **Cache control.** Scattering tiles by hashed grid index across the whole 396 MiB atlas costs the same: 2.36 / 3.16 / 2.70 against 2.34 / 3.18 / 2.69 (one run). Inferred, not measured: the cost is per-sample shader work, not memory.
- **Real tiles.** One run with baked tiles reads 2.33 / 3.15 / 2.66 at 6×6 and 2.34 / 3.22 / 2.77 at 8×8, against 2.12 / 3.07 / 2.50 tiles off in the same batch. This agrees with the synthetic numbers.

Limits: 1660 only, one hallway position, `dev` profile rather than release, and `forward` alone, though fog, fluid and billboard use the same sampler.

### Q3: how many probes need a tile

**Definition.** For each valid probe, compare the sampler's visibility, isotropic against tile, toward its 26 lattice neighbours at the neighbour's distance, with shipping floor and bias. A probe counts if visibility differs by more than 0.1 toward a valid neighbour. This is a full count, not a sample: a depth-only trace of 962,636 probes takes 63 s and exits before the SH gather (`PDD_Q3`), so no rebake was needed. The current map and the 10-31 PRL agree on validity for all 2,212,901 grid probes.

| Tile | Probes | Share of valid | Level 0 (679,176) | Level 1 (230,137) | Level 2 (53,323) | Any neighbour, valid or not |
|---|---|---|---|---|---|---|
| 4×4 | 5,828 | 0.61% | 4,610 | 1,214 | 4 | 24.8% |
| 6×6 | 16,481 | 1.71% | 13,246 | 3,211 | 24 | 29.6% |
| 8×8 | 13,451 | 1.40% | 10,925 | 2,508 | 18 | 29.6% |

The levels are `density_level`. By reading the sampler, levels 0 and 2 read one slot and level 1 reads eight corner slots; I did not convert these to a slot count. Nearly every change makes a neighbour darker: no probe got brighter by more than 0.1, because the isotropic test almost never rejects at this distance. The last column counts directions into solid, where no receiver sits, so it overstates need.

**Does the flag agree with the geometry?** Each valid-neighbour direction was checked against a traced probe-to-neighbour segment.

| Map, tile | Blocked directions flagged | Clear directions flagged |
|---|---|---|
| Closet, 4×4 | 133 of 172 (77%) | 6 of 35,330 |
| Closet, 6×6 | 152 of 172 (88%) | 11 of 35,330 |
| Closet, 8×8 | 160 of 172 (93%) | 10 of 35,330 |
| Hallway, 4×4 | 5,598 of 38,268 (15%) | 3,693 of 21,847,088 |
| Hallway, 6×6 | 13,435 of 38,268 (35%) | 18,071 of 21,847,088 |
| Hallway, 8×8 | 16,074 of 38,268 (42%) | 13,126 of 21,847,088 |

On the closet, all 35 probes with a blocked valid neighbour are flagged at every size, so the metric finds the probes that leak. On the hallway, 13,802 probes (1.43%) have a blocked valid neighbour and tiles catch a minority of those directions, and 40–57% of flags are on clear segments. Why is not isolated. A centre-line segment can be blocked by a small occluder or an edge that most of a texel's rays pass, which would make these misses harmless, but that is a guess. No leak was observed at these probes.

**False flags on the real map.** Hallway captures with baked tiles against tiles off, at the three Q2 views: no pixel darkens by more than 2 levels at 6×6 or 8×8, and at most 11 of 2,073,600 pixels brighten by more than 2 (8×8). That is one room, not the map.

**Sparse tiles, tested on the closet only.** `kit/win/sparse.py` keeps tiles only on Q3-flagged probes and gives every other probe its isotropic mean. With 40 of 3,780 tiles at 8×8 and 41 at 6×6, static-door captures are pixel-identical to giving every probe a tile. The no-door map keeps 23 tiles and reads 29.87 / 15.03. Sampling was not changed, so a real sparse lookup's cost is unmeasured.

**Memory, computed, not measured.** Tiles for all of about 907k slots, with a 1-texel border: 125 / 221 / 346 MiB at 4×4 / 6×6 / 8×8. Flagged probes only: 0.8 / 4.0 / 5.1 MiB, before any lookup structure.

### Findings

Measured against the questions, pending the owner's glow verdict:

- **Q1.** Per-direction moments remove most of the sampler's glow and leave the open doorway alone. The isotropic control ties the effect to direction.
- **Q2.** About 0.2–0.35 ms on the forward pass on the 1660, independent of tile size.
- **Q3.** About 1.4–1.7% of hallway probes change outcome. If sparse storage holds up, memory stops being the constraint.

**Recommendation.** If the owner accepts the glow, take per-direction moments to `/draft-session`, with 8×8 sparse tiles as the candidate. It has the lowest residual, the best agreement with geometry, and costs the same as smaller tiles. If tiles must cover every slot, memory argues for a smaller size, and 4×4 leaves about three times the residual of 8×8.

**Open before a spec:**

- **A texture slot on Metal.** This build uses a 17th; Metal's limit is 16.
- **IP review of sparse tiles.** Deciding at bake time which probes carry a tile is not a runtime visibility test, and every probe keeps a continuous weight. But it is a per-probe selection, so it needs the owner's check against `ip-check.md` before anyone builds it.
- **Sparse lookup cost.** Unmeasured, as is sparse behaviour on the hallway.
- **The remaining outline.** Mostly the 3% visibility floor, and the rest is unexplained.
- **Hallway disagreement with geometry.** See Q3.
- **Closed mover doors.** Still unsolved, as the README states: tiles are baked with movers absent.

### Hacks in this code

Every site carries a `SPIKE HACK` comment.

- **17th sampled texture.** Tiles are a new binding (30) on the shared SH layout, and `FORWARD_SAMPLED_TEXTURE_BUDGET` is raised from 16 to 17. No slot was borrowed. **Metal's ceiling is 16, so this cannot run on the Mac, and all Q2 figures are 1660-only until a slot is found.**
- **Sidecar, no format work.** `prl-build` writes tiles beside the map when `PDD_OUT` is set, from its own depth-only trace (`sh_bake/pdd.rs`). Radiance is untouched, so the 1,024-ray runs vary depth only. The engine reads the file named by `POSTRETRO_PDD_TILES` and aborts if its grid differs from the level's.
- **No streaming.** The closet's tiles are one resident atlas in linear probe order.
- **Q2 cost modes.** `POSTRETRO_PDD_Q2_T` fills tiles with noise that always passes the test, one per SH slot, and `POSTRETRO_PDD_Q2_SCATTER` hashes grid indices across the atlas. The Q2 matrix ran on a build whose scatter hashed the slot; the scatter figures above are from the corrected build, which differs only in that branch.
- **Q3 pass.** `PDD_Q3` in `prl-build` counts, writes one byte per probe, and exits. `POSTRETRO_PDD_Q3_BITS` logs that count by density level at load.
- **Spare uniform words.** `ShGridInfo._pad0` holds the tile size, `_pad1` the floor override, `_pad3` the Q2 mode, and the unread `atlas_tile_rows` the bias override.
- **Tile format.** `Rg16Float`, one bordered tile per probe, bilinear through the existing atlas sampler. A ray is binned into its texel, with no cosine-weighted filter. E[d²] in f16 has the same precision limit as today's moments.
- **Fixture.** `gen_closet.py` gained `--no-door`.

IP: every probe keeps a continuous weight. No probe is excluded, and no mask or binary visibility decision was added.

## Mac

Radeon Pro 5300M, Metal, hallway Q2 views a / b / c, base 2968de0ad's parent (bc5af2dac). Summary only; raw runs and the measurement shader patch were discarded. The brief's `research.md` (`direction-aware-probe-visibility`) carries the tables that set its ceiling.

- **Method.** `POSTRETRO_GPU_TIMING` is unsupported on this adapter (no timestamp features), so forward time is the `Textured Pass` encoder's per-frame time from a Metal System Trace (`xctrace record --launch`), extracted by `kit/mac/q2mac.py` and `mst_rows.py`. Measure at 4K: at low resolution capture mode lets the GPU clock fall and runs swing by milliseconds. At 720p, alternate configurations every frame in one process and take the median back-to-back difference. `xctrace record` sometimes hangs after the engine exits; the script's watchdog kills and reruns that run. Keep `caffeinate` on and check for the screen saver per run.
- **First lookup, every probe tiled:** +6.3 to +7.8 ms forward at 4K (23–40%), any tile size. The fetch was about 1.2 ms of it; per-corner `normalize`, texture-size query and runtime-width integer divide carried the rest.
- **Cheap lookup** (no normalize, constants instead of the size query, power-of-two row with shifts): every probe tiled +2.6 to +3.3 ms; sparse (hashed 1.7% of slots) +1.13 to +1.19 ms (4–6%).
- **Declared-texture floor:** +0.85 to +1.02 ms of the sparse cost is paid with no tile read; naga's Metal writer declares globals as entry arguments and the AMD driver charges per argument.
- **720p, paired:** sparse +0.01 to +0.06 ms over no tile read; every probe tiled +0.28 to +0.40 ms.
- **Binding:** the tiles cannot replace the depth-moments texture (its B/A carry the probe indirection word). They take a new binding while the direct-SH atlas drops fragment visibility in the shared SH layout, keeping forward at 16 sampled textures.
