# Glossy world reflections, Task 10: water GPU byte identity, 5300M

Task 10 rewrote the WGSL water probe search into the generic hemisphere-axis form (water passes a constant +Y axis). This check compares real-map frames from the binary before Task 10 with HEAD's binary, which also carries the `origin/main` merge.

- **Old:** release build of 25a74b0d1, `capture,dev-tools` (pre-Task-10, pre-merge).
- **New:** release build of 32254d1b6 (HEAD: Task 10 plus the `origin/main` merge), `capture,dev-tools`.
- **Maps:** `content/dev/maps/reflection-probes-test.map` and `content/dev/maps/reflection-lake-test.map`, each baked by the new `prl-build --no-cache`, once without `--glossy-reflections` and once with it. With and without the flag, each map's `.prl` is byte-identical (no glossy faces), so the two bakes give the same frames.
- **Capture:** plain PNG capture (`--capture <scene>`, no measurement block): every frame renders the authored instant at time zero (`rendering_pipeline.md` §7.8). The water clock is therefore pinned and identical across binaries. 1920x1080, FOV 90, `POSTRETRO_SH_STREAMING=sync-proof`, both binaries reading the same `.prl` files.
- **Quality:** off, low (probe term only), high (probe + screen-space), very_high (probe + planar). Style pixel (the capture default).

## Poses (engine coordinates, metres)

| pose | map | position | yaw | pitch | view |
|---|---|---|---|---|---|
| probes-down | probes | (-6.5024, 1.6002, -6.5024) | 0 | -89 | pool, straight down (the kit's `test` pose) |
| probes-across | probes | (-2.032, 1.6002, -6.5024) | 90 | -35 | pool from its south edge, toward the sign |
| probes-canal | probes | (-6.5024, 1.6256, -25.4) | 0 | -40 | room B canal and basin, east toward the pillar |
| lake-spawn | lake | (-17.78, 1.6002, -48.768) | 90 | -25 | south shore across the lake |
| lake-centre | lake | (-43.18, 3.81, -46.3296) | 90 | -30 | over the lake toward the centre lamp |
| lake-lamp | lake | (-35.56, 1.524, -40.64) | 270 | -50 | over the lake near the south-lake lamp |

## Result

All 48 old/new pairs (54 with the smooth-style pairs below) are byte-identical PNGs: 6 poses x 4 qualities x 2 bakes. Pixels differing: 0; maximum difference: 0. The merged `origin/main` changes draw nothing in these frames. Capture is VM-free, with no player body or weapon.

The probe term is live in these frames, so identity is not vacuous. New binary, without-flag bake; pixels of 2,073,600 differing between tiers, with the maximum channel difference:

| pose | low vs off (probe term) | high vs low | very_high vs high |
|---|---|---|---|
| probes-down | 2,038,635 (max 18) | 0 | 990,519 (max 12) |
| probes-across | 473,622 (max 73) | 53,187 (max 73) | 449,295 (max 192) |
| probes-canal | 1,685,999 (max 33) | 106,033 (max 30) | 909,059 (max 45) |
| lake-spawn | 736,593 (max 103) | 128,842 (max 126) | 269,667 (max 133) |
| lake-centre | 1,554,969 (max 82) | 157,262 (max 128) | 301,001 (max 127) |
| lake-lamp | 1,544,316 (max 40) | 415 (max 1) | 192,967 (max 29) |

probes-down's high frame equals its low frame: looking straight down, the screen-space layer adds nothing.

Smooth style: the six poses were also captured at high with `reflection_style: smooth` (the without-flag bake). All 6 old/new pairs are byte-identical. Smooth differs from pixel (probes-across 247,026 pixels, max 63; lake-spawn 45,992, max 125), so the smooth search path ran too.

Artefacts (scratch, not kept): scenes, PNGs and logs under the session scratchpad's `task10meas/water/`.
