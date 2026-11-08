# direction-aware-probe-visibility — research

Read at 2968de0ad. Spike: branch `spike/per-direction-depth`, `context/research/per-direction-depth-spike/README.md` (Windows), plus the Mac sections on `measure/pdd-mac-timing` (dfcc67fd9).

## Cause
`sh_corner_depth_visibility` (`sh_sample.wgsl`) reads one `(E[d], E[d²])` pair per dense lattice probe from `sh_depth_moments` (`Rgba16Uint` 3D; R/G the f16 moments, B/A the indirection word). The bake (`bake_probe_with_source` → `bake_probe_rgb_along`, `sh_bake/probe.rs`) averages 256 fixed Fibonacci rays from the lattice point; sky misses count as `4·|cell_size|`. A probe 0.09 m from a closed door has a long average, because most rays cross the bright room, so closet points 0.8 m behind the door pass Chebyshev.

## Spike (Windows, GTX 1660 SUPER, 256 rays)
Static-door reference closet; station 3 mean, station 2 mean, then mean brightness in floor bands 0.04 / 0.26 / 0.48 m behind the door.

| Sampler | St3 | St2 | 0.04 m | 0.26 m | 0.48 m |
|---|---|---|---|---|---|
| Today | 0.50 | 0.15 | 11.0 | 1.8 | 0.0 |
| 4×4 | 0.13 | 0.03 | 1.3 | 0.0 | 0.0 |
| 6×6 | 0.05 | 0.01 | 0.7 | 0.0 | 0.0 |
| 8×8 | 0.04 | 0.01 | 0.6 | 0.0 | 0.0 |

- Isotropic control reproduces today exactly; the tile path is the cause of the change.
- Open doorway: frame means 29.86 / 15.03–15.04 at every size against 29.86 / 15.04 today. Worst: 445 of 518,400 pixels move by more than 2 levels (8×8, station 2).
- Floor and bias at 6×6: floor 0.003 alone gives 0.03 / 0.01, band 0.2; bias 0 alone changes nothing; both give band 0.1. A frame-mean residue of 0.03 and a thin door-edge outline (visible at 8× gain) remain, source not isolated.
- Noise: 1,024 rays move frame means and bands by at most 0.01.
- Hallway false flags: no pixel darkens by more than 2 levels at 6×6 or 8×8; at most 11 of 2,073,600 brighten.
- Selection (Q3): a valid probe counts when its visibility toward any of its 26 lattice neighbours, at the neighbour's distance and with the shipping floor and bias, differs by more than 0.1 between averaged and tile moments. Hallway, 962,636 valid probes: 4×4 5,828 (0.61%), 6×6 16,481 (1.71%), 8×8 13,451 (1.40%). All 35 closet probes with a blocked valid neighbour are flagged at every size.
- Cost: forward +0.16–0.35 ms (5–17%) at 1080p, independent of tile size. The spike bound a 17th fragment texture, which Metal refuses.

## Mac (Radeon Pro 5300M, Metal)
`POSTRETRO_GPU_TIMING` is unsupported on this adapter; timing comes from Metal System Trace (`Textured Pass` encoder). 4K, because capture mode lets the clock fall at low resolution. Hallway Q2 views a / b / c.

| Forward ms, 4K | a | b | c |
|---|---|---|---|
| Today | 19.58 | 27.77 | 25.04 |
| Patched, no tile read | 20.43 | 28.78 | 25.97 |
| Cheap lookup, sparse (hashed 1.7%) | 20.71 | 28.96 | 26.20 |
| Cheap lookup, every probe | 22.83 | 30.44 | 27.65 |
| First lookup, every probe | 27.37 | 34.15 | 31.32 |

- Today, no-tile-read and sparse rows are means of 3 runs; the rest single runs. Sparse over today: +1.13 / +1.19 / +1.16 ms (4.3–5.8%); the declared texture alone is +0.85 / +1.02 / +0.93. A paired in-process run on view a agrees for sparse (+0.28 over no tile read).
- Ceiling: the worst sparse view, +1.19 ms, plus 25% gives +1.5 ms per view. Unpriced in that run: reading the tile reference from the moments texel (already fetched) and the 8×8 bordered stride (one multiply); tile size made no difference.
- 720p, paired per frame because the clock swings between runs: sparse adds +0.01 / +0.06 / +0.04 ms over no tile read; every probe tiled adds +0.28 / +0.40 / +0.30. The declared-texture floor is not measurable at 720p this way. Reference only.
- First lookup: the fetch was about 1.2 ms of +7.8 (measured). Attributing the rest to the per-corner `normalize`, texture-size query and runtime-width integer divide is inferred: the cheap-lookup binaries differ only by dropping those three (`measure/pdd-mac-timing` dfcc67fd9).
- About 1 ms of the sparse cost is the declared texture itself, paid with no tile read: naga's Metal writer declares globals as entry arguments and the AMD driver charges per declared argument (`rendering_pipeline.md` §7.3a, Two pipelines).
- Hashed sparse selection scatters tiles; real tiles cluster at thin walls, so divergence should be no worse.
- Run-to-run spread at 4K ≤ 0.32 ms; one run per cell suffices there.

## Binding
- Shared SH group (`sh_bind_group_layout_entries`): forward, billboard, fluid, fog. Mesh superset (`mesh_bind_group_layout_entries`, the same entries plus binding 16): kinematic and skinned at group 4.
- Forward fragment textures: 16 with cube arrays, 15 without (`FORWARD_SAMPLED_TEXTURE_BUDGET`, `forward_pipeline_sampled_texture_request_matches_bgl_definitions`). The shared group contributes total atlas, depth moments and direct atlas.
- `sh_direct_atlas` in the shared layout: forward and fog do not declare it; billboard and fluid read it only from `vs_main`. Kinematic and skinned read it in `fs_main` through the superset.
- Vertex inventories after the change: billboard and fluid group 3 go from 4 to 5 sampled textures (limit 16). Kinematic and skinned fragment: 11 → 12 with cube arrays.
- Indirection word (`sh_indirection.wgsl`): level bits 0–1, valid bit 2, scale bits 3–4, slot bits 5–31. No spare bit. R and G hold `E[d]` and `E[d²]` as f16, both non-negative by construction; the id-34 loader does not validate them today.
- `sdf_shadow.wgsl` decodes R raw as `E[d]` for its open-space skip, through the same moments view; keeping R untouched leaves it unchanged.

## Memory
- Hallway at 8×8: 13,451 tiles × 10×10 bordered texels × 4 B ≈ 5.1 MiB resident and on disk; ceiling 6.4 MiB. At 6×6 it would be 16,481 tiles, ≈ 4.0 MiB.
- Cap: 32,768 tiles per level (indices 0–32,767), the sign-flagged reference's limit in a 16-bit channel; ≈ 12.5 MiB at 8×8. Decode tests the raw sign bit: tile 0 is f16 −0 and the top indices are −inf or NaN as floats.
- Tiles on every probe instead: 125–346 MiB across tile sizes. The measurement hack's per-slot layout: 67 MB at 6×6.
- Existing dense moments texture: 8 B per lattice probe, whole-resident.

## Fixture
- `mover-closet` map and README are on `main`; its static-door and no-door variants come from `kit/gen_closet.py` on `spike/per-direction-depth`.
- Stations, map units: 2 at (320, 720, 64) yaw 270, inside the closet facing the door; 3 at (320, 610, 96) yaw 270 pitch −38, the closet floor at the door.
- Capture: feature `capture`, `POSTRETRO_SH_STREAMING=sync-proof`, scene field `force_full_resident_sh_compose`, 960×540. Scene JSON as in `measurements/surface-radiance-cache/ab-captures/scenes/`.
- Do not move the closet's north wall: y 760 is the worst measured lattice phase.

## Doors this leaves open
- Per-mover tiles: closed-pose tiles reach a probe through the CPU reference table. A step switch on dock suits a door that snaps shut; a glow that ramps with door travel needs a runtime-composed tile blending both moment sets by travel. The probe channel decides.
- Relocation (roadmap, Majercik 2021): moving a probe off a wall moves its bake position; irradiance, moments and tiles follow together.

## Measure
- No-tile PRL: the tiled bake's PRL with id 59 removed and the section table rewritten, so every other section is byte-identical; the Automated "today" rows and the Manual cross-build row load that file.
- Frame mean and pixel moves are Rec. 709 luma of the post-tonemap sRGB PNG (0–255), as the spike's `pngstat.py` and `pngdiff.py`; a "move" is |Δluma| > 2. Closet 960×540; hallway Q2 views 1920×1080.

## Orderings

| Id | Scenario | Ordering | Expected |
|---|---|---|---|
| O1 | Warm hit | Cold build writes `sh_base_volume`; second build of unchanged inputs hits it | Same id 59 bytes as the cold build; dropped count logged again |
| O2 | Stale cache | Entry written by the read-at compiler; tiled compiler builds the same inputs | Entry misses (stage version moves); tiles emitted |
| O3 | Texel writers | Whole-load pack; streamed promote; evict then promote; session clear then promote | Each written texel of a tiled valid probe decodes to its tile |
| O4 | Pool growth | Tiles resident; mandatory demand grows the dense pool; next frame samples | Rebuilt groups bind the level's tile texture and tile-row constant; visibility unchanged across the growth frame |
| O5 | Same-drain race | Reference rewrite and cluster promote for one probe queued in one drain, both orders | Rewrite wins in both orders |
| O6 | Redirect then evict | Rewrite; cluster evicted; cluster reinstalled | Texel decodes to the rewritten tile |
| O7 | Clear | Probe with tile; reference cleared | Averaged moments restored; visibility equals today's bit for bit |
| O8 | Second reader | Tiled probe; SDF open-space skip reads the same texel that frame | Reads a non-negative distance; shadow factor unchanged by the reference |
| O10 | Level change | Rewrite on the published load; build a second load of the same map, publish it, then reload | Each load's texels decode to its own id 59; no rewrite carries across |
| O9 | Cap ties | Candidates with equal visibility change straddle the cap | Lower probe index kept; same set on every build |
