# direction-aware-probe-visibility — research

Read at 2968de0ad. Spike: branch `spike/per-direction-depth`, `context/research/per-direction-depth-spike/README.md` (Windows), plus the Mac sections on local branch `measure/pdd-mac-timing` (to be pushed beside it).

## Cause
`sh_corner_depth_visibility` (`sh_sample.wgsl`) reads one `(E[d], E[d²])` pair per dense lattice probe from `sh_depth_moments` (`Rgba16Uint` 3D; R/G the f16 moments, B/A the indirection word). The bake (`bake_probe_rgb_with_moments`, `sh_bake/probe.rs`) averages 256 fixed Fibonacci rays from the lattice point; sky misses count as `4·|cell_size|`. A probe 0.09 m from a closed door has a long average, because most rays cross the bright room, so closet points 0.8 m behind the door pass Chebyshev.

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
| Today | 19.6 | 27.8 | 25.0 |
| Patched, no tile read | 20.5 | 28.8 | 26.0 |
| Cheap lookup, sparse (hashed 1.7%) | 20.8 | 29.0 | 26.2 |
| Cheap lookup, every probe | 22.8 | 30.5 | 27.7 |
| First lookup, every probe | 27.4 | 34.2 | 31.4 |

- These rows are interim, read from the second run's log; the checked report and 720p reference figures replace them before promotion.
- First lookup: the fetch was about 1.2 ms of +7.8; per-corner `normalize`, texture-size query and integer divide by a runtime row width carried the rest.
- About 1 ms of the sparse cost is the declared texture itself, paid with no tile read: naga's Metal writer declares globals as entry arguments and the AMD driver charges per declared argument (`rendering_pipeline.md` §7.3a, Two pipelines).
- Hashed sparse selection scatters tiles; real tiles cluster at thin walls, so divergence should be no worse.
- Run-to-run spread at 4K ≤ 0.32 ms; one run per cell suffices there.

## Binding
- Shared SH group (`sh_bind_group_layout_entries`): forward, billboard, fluid, fog. Mesh superset (`mesh_bind_group_layout_entries`, the same entries plus binding 16): kinematic and skinned at group 4.
- Forward fragment textures: 16 with cube arrays, 15 without (`FORWARD_SAMPLED_TEXTURE_BUDGET`, `forward_pipeline_sampled_texture_request_matches_bgl_definitions`). The shared group contributes total atlas, depth moments and direct atlas.
- `sh_direct_atlas` in the shared layout: forward and fog do not declare it; billboard and fluid read it only from `vs_main`. Kinematic and skinned read it in `fs_main` through the superset.
- Vertex inventories after the change: billboard and fluid group 3 go from 4 to 5 sampled textures (limit 16). Kinematic and skinned fragment: 11 → 12 with cube arrays.
- Indirection word (`sh_indirection.wgsl`): level bits 0–1, valid bit 2, scale bits 3–4, slot bits 5–31. No spare bit. R holds `E[d]` as f16, which is non-negative, so its sign bit can flag a tile reference spread across R and G.

## Memory
- Hallway at 6×6: 16,481 tiles × 8×8 texels × 4 B ≈ 4.2 MB resident and on disk. Ceiling 5.3 MiB.
- Tiles on every probe instead: 125–346 MiB across tile sizes. The measurement hack's per-slot layout: 67 MB at 6×6.
- Existing dense moments texture: 8 B per lattice probe, whole-resident.

## Fixture
- `mover-closet` map and README, with static-door and no-door variants (`kit/gen_closet.py`), live only on `spike/per-direction-depth`.
- Stations, map units: 2 at (320, 720, 64) yaw 270, inside the closet facing the door; 3 at (320, 610, 96) yaw 270 pitch −38, the closet floor at the door.
- Capture: feature `capture`, `POSTRETRO_SH_STREAMING=sync-proof`, scene field `force_full_resident_sh_compose`, 960×540. Scene JSON as in `measurements/surface-radiance-cache/ab-captures/scenes/`.
- Do not move the closet's north wall: y 760 is the worst measured lattice phase.

## Doors this leaves open
- Per-mover tiles: the mover channel bakes tiles for a door's closed pose and rewrites the reference in the moments texel while the door rests closed.
- Relocation (roadmap, Majercik 2021): moving a probe off a wall moves its bake position; irradiance, moments and tiles follow together.
