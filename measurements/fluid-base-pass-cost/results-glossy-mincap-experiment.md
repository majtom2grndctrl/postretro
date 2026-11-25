# Glossy world reflections: minimum-blur cap experiment (mip 2 vs 1 vs 0)

Scratch experiment, not a plan row. Question: what lowering `REFLECTION_PROBE_MIN_BLUR_MIP` from 2 (today) to 1 or 0 changes in look and in GPU cost. The cap clamps `reflection_probe_exponent_mip(n)` = log₂(θ(n)/α₀) from below; the glossy overlay's probe term and the floor screen-space layer's blur both call it, so both layers move together. Natural mips: n ≈ 143 reads mip 2, n ≈ 575 mip 1; the exponent cap 8192 reads mip −0.92, so at cap 0 mip 0 is the floor.

- **Binaries.** cap 2: the HEAD-equivalent release engine (`capture,dev-tools`) that Task 11 / 12 measured. cap 1 and cap 0: HEAD `61fbf5969` with the Rust constant (`crates/level-format/src/reflection_probes/blur.rs`) and its WGSL twin (`crates/renderer/src/shaders/reflection_probe_blur.wgsl`) edited, built `--release -p postretro --features capture,dev-tools` in scratch target dirs, binaries copied out, source restored by editing back, targets deleted. Nothing committed.
- **Bake is cap-independent.** A cap-0 `prl-build` (same edit) and the cap-2 compiler, both `--no-cache --glossy-reflections`, produce byte-identical PRLs for the showcase map (32 sections) and for the floor-only ceiling fixture (29 sections), and identical `.prm` sidecars; the fresh cap-2 showcase bake is also byte-identical to the on-disk one. (An earlier check found the same for cap 1.) So every binary renders one shared bake: the cap reaches only the runtime mip choice.
- **Measurement.** `run_ablation.py --report glossy` through a scratch wrapper that adds variants `cap2` / `cap1` / `cap0` (each its own binary, all under the kit's `after` rules: a complete frame requires the overlay and the floor march) and a `glossy-showcase-hero` pose (the showcase hero camera, fov 90). One batch, all three binaries interleaved, 2 rounds (order reversed in round 2), 300 warm-up + 300 sampled frames, mask 17F, quality High, default specular style. Map root: the floor-only fixture (`maps/glossy-floor-only.map` baked as `glossy-reflections-test.prl`) and the showcase map, one shared bake. Pilot first (4 runs, `--abort-invalid`; `results-glossy-mincap-pilot.md`). The 5300M MacBook Pro, nothing else building.

## What the cap reaches

The fixture's floor, `test/metal_smoothness_ramp` (16×16, `_s` 255 everywhere), stores smoothness byte 17·x in column x. Decoded (n = 2/r⁴ − 2, clamped 1..8192) and mapped to a mip:

| column | byte | n | natural mip | cap 2 reads | cap 1 reads | cap 0 reads | reflection weight |
|---|---|---|---|---|---|---|---|
| 0–1 | 0, 17 | 1 | 5.42 | 5.42 | 5.42 | 5.42 | 0 |
| 2 | 34 | 1.5 | 5.16 | 5.16 | 5.16 | 5.16 | 0 |
| 3 | 51 | 2.9 | 4.76 | 4.76 | 4.76 | 4.76 | 0 |
| 4 | 68 | 4.9 | 4.40 | 4.40 | 4.40 | 4.40 | 0 |
| 5 | 85 | 8.1 | 4.05 | 4.05 | 4.05 | 4.05 | 0.02 |
| 6 | 102 | 13.4 | 3.70 | 3.70 | 3.70 | 3.70 | 0.75 |
| 7 | 119 | 22.7 | 3.32 | 3.32 | 3.32 | 3.32 | 1 |
| 8 | 136 | 40.2 | 2.92 | 2.92 | 2.92 | 2.92 | 1 |
| 9 | 153 | 76.1 | 2.46 | 2.46 | 2.46 | 2.46 | 1 |
| 10 | 170 | 160 | 1.92 | **2** | 1.92 | 1.92 | 1 |
| 11 | 187 | 394 | 1.27 | **2** | 1.27 | 1.27 | 1 |
| 12 | 204 | 1248 | 0.44 | **2** | **1** | 0.44 | 1 |
| 13 | 221 | 6326 | −0.73 | **2** | **1** | **0** | 1 |
| 14–15 | 238, 255 | 8192 | −0.92 | **2** | **1** | **0** | 1 |

The ramp spans n 1 → 8192 (natural mip 5.4 → −0.9). Bold = clamped by the cap. Cap 2 binds on 6 of 16 columns (n ≥ 160), cap 1 on 4 (n ≥ 1248), cap 0 on 3 (n ≥ 6326, where it reads mip 0). Going 2 → 1 changes the mip read on 6/16 of the floor's texels; 1 → 0 on 4/16 (columns 12–15). Every column still costs the same overlay and march work at any cap: the cap moves the sampled mip, not the number of fetches.

On the showcase, the sharp floor zone `test/metal_mirror_tinted` (byte 200, n ≈ 922, natural mip 0.66) is clamped by caps 2 and 1 and reads its natural 0.66 at cap 0; the rough zone `test/metal_satin_tinted` (byte 115, n ≈ 20, mip 3.41) is unaffected. The north wall `test/concrete_carved_tinted_gloss` stores byte 235 (n = 8192, mip −0.92) on 72 of its 256 texels (the lighter cyan tiles) and byte 120 (n ≈ 23, mip 3.3) on 72 more, so its mirror-byte tiles also move with the cap; the expectation that only the floor's sharp zone changes misses that wall.

## Cost

36/36 runs valid (pilot 4/4). Per complete frame, mean of 2 runs; spread = max − min of the two; Δ = against cap 2 in the same batch.

| pose | res | cap | valid | overlay ms | spread | Δ vs cap 2 | march ms | spread | Δ vs cap 2 | forward ms | GPU frame ms | Δ frame |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| across | 1440 | 2 | 2/2 | 2.046 | 0.003 | — | 1.598 | 0.010 | — | 11.231 | 16.620 | — |
| across | 1440 | 1 | 2/2 | 2.050 | 0.003 | +0.003 | 1.595 | 0.004 | -0.002 | 11.207 | 16.592 | -0.028 |
| across | 1440 | 0 | 2/2 | 2.048 | 0.011 | +0.002 | 1.595 | 0.004 | -0.002 | 11.233 | 16.595 | -0.025 |
| across | 4k | 2 | 2/2 | 4.536 | 0.009 | — | 3.586 | 0.017 | — | 25.040 | 35.933 | — |
| across | 4k | 1 | 2/2 | 4.538 | 0.008 | +0.002 | 3.589 | 0.005 | +0.003 | 25.050 | 35.968 | +0.035 |
| across | 4k | 0 | 2/2 | 4.533 | 0.002 | -0.003 | 3.594 | 0.002 | +0.007 | 25.045 | 35.957 | +0.024 |
| down | 1440 | 2 | 2/2 | 3.051 | 0.004 | — | 1.886 | 0.002 | — | 10.212 | 16.783 | — |
| down | 1440 | 1 | 2/2 | 3.049 | 0.003 | -0.002 | 1.886 | 0.003 | -0.000 | 10.204 | 16.772 | -0.011 |
| down | 1440 | 0 | 2/2 | 3.050 | 0.000 | -0.002 | 1.885 | 0.005 | -0.002 | 10.223 | 16.756 | -0.027 |
| down | 4k | 2 | 2/2 | 6.792 | 0.002 | — | 4.128 | 0.004 | — | 23.340 | 36.703 | — |
| down | 4k | 1 | 2/2 | 6.793 | 0.008 | +0.001 | 4.135 | 0.004 | +0.007 | 23.334 | 36.690 | -0.013 |
| down | 4k | 0 | 2/2 | 6.806 | 0.014 | +0.014 | 4.127 | 0.002 | -0.001 | 23.348 | 36.718 | +0.015 |
| showcase-hero | 1440 | 2 | 2/2 | 2.778 | 0.003 | — | 1.472 | 0.000 | — | 17.836 | 25.018 | — |
| showcase-hero | 1440 | 1 | 2/2 | 2.778 | 0.011 | +0.000 | 1.471 | 0.004 | -0.002 | 17.841 | 25.031 | +0.013 |
| showcase-hero | 1440 | 0 | 2/2 | 2.781 | 0.001 | +0.003 | 1.472 | 0.004 | -0.001 | 17.838 | 25.016 | -0.002 |
| showcase-hero | 4k | 2 | 2/2 | 6.103 | 0.002 | — | 3.185 | 0.004 | — | 39.523 | 53.717 | — |
| showcase-hero | 4k | 1 | 2/2 | 6.107 | 0.000 | +0.004 | 3.187 | 0.001 | +0.002 | 39.523 | 53.724 | +0.007 |
| showcase-hero | 4k | 0 | 2/2 | 6.104 | 0.002 | +0.001 | 3.188 | 0.009 | +0.003 | 39.546 | 53.745 | +0.028 |

**Read.** No cost difference at any pose, resolution or cap. Every overlay and march Δ is within ±0.014 ms, the same size as the arms' own run spreads (up to 0.017 ms), and has no consistent sign; whole-frame Δ is within ±0.035 ms on 16.6–53.7 ms frames. Busy core clocks were 1057–1213 MHz (run medians) across the batch. That is what the shader predicts: the cap only changes the mip level fed to the same `textureSampleLevel` calls, so the overlay and the march issue identical work, and reading a finer mip of a 128² tile is not measurably costlier on this GPU at these sizes. The showcase hero view, whose sharp zone is the case the cap is for, is as flat as the fixture.

## Look

Captures through `--capture` (High; smooth and pixel), the showcase map's one shared bake, 1920×1080; every capture is deterministic (cap 2 re-captured byte-identical to the earlier showcase capture; cap 1 byte-identical to the earlier cap-1 capture). Side-by-sides (cap 2 | 1 | 0) and crops are in the session scratchpad (`mmin/compare-*.png`, `mmin/crop-*.png`, `mmin/zoom-*.png`, `mmin/diffs-hero.png`); not committed.

- **Where it changes.** Difference maps confirm only the sharp floor zone and the north wall's mirror-byte tiles change, 2 → 1 and 1 → 0; the rough floor zone, every matte surface and the wall's other tiles are identical. Changed pixels 1 → 0: 0.5% of the hero frame, 0.9% of the grazing frame (2 → 1: 1.2% and 2.0%).
- **2 → 1.** The sharp zone's reflection gains real structure: the glowing pillar and the stripe column read as distinct vertical streaks, the monument's reflection firms up, and the light bar's reflection appears as a soft bright patch. Still clearly a blurred probe reflection; no new artefact seen.
- **1 → 0.** A further, smaller step with one new artefact. The reflected light bar collapses into a bright horizontal band whose edge is visibly scalloped: a regular lumpy edge (about 30 px period at 1080p in the grazing view, about 12 px in the hero view) that is the probe's mip-0 texels (≈ 2.8° each) magnified under bilinear filtering. It reads as blockiness of the 128² tile, not as a smooth edge, and in motion it would slide and crawl with the view (not judged here: stills only). The floor's checker cells also show faint texel-scale mottling in the difference map. The north wall's mirror tiles go from a soft reflection to a legible, hard-edged image of the room. No octahedral seam or sparkle was visible in these frames; the brightest content is a large emissive bar, so single-texel sparkle had little to show on.
- **Pixel style** behaves the same as smooth in the hero view (the reflection term does not quantize differently between caps).

## Files

Run files: `metal/runs/glossy-mincap-{pilot,experiment}-*` (untracked). Kit summaries: `results-glossy-mincap-pilot.{md,json}`; this table's data: `results-glossy-mincap-experiment.json`. The scratch wrapper (`mincap_ablation.py`) and driver are not in the repo.

### Kit table

| arm | valid | overlay ms | spread | sums ms | chain ms | march ms | mover ms | spread | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-1440-cap2-m17f-high | 2/2 | 2.046 | 0.003 | 0.014 | 0.274 | 1.598 | 0.000 | 0.000 | 11.231 | 0.004 | 16.620 | 0.007 | 16.657 | 0.037 | 1099 (885-1169, n=21); 1103 (925-1155, n=20) |
| glossy-across-1440-cap1-m17f-high | 2/2 | 2.050 | 0.003 | 0.014 | 0.294 | 1.595 | 0.000 | 0.000 | 11.207 | 0.055 | 16.592 | 0.034 | 16.628 | 0.036 | 1088 (920-1165, n=21); 1083 (896-1209, n=20) |
| glossy-across-1440-cap0-m17f-high | 2/2 | 2.048 | 0.011 | 0.014 | 0.281 | 1.595 | 0.000 | 0.000 | 11.233 | 0.071 | 16.595 | 0.024 | 16.630 | 0.035 | 1106 (881-1199, n=21); 1057 (812-1181, n=21) |
| glossy-across-4k-cap2-m17f-high | 2/2 | 4.536 | 0.009 | 0.013 | 0.501 | 3.586 | 0.000 | 0.000 | 25.040 | 0.030 | 35.933 | 0.066 | 35.965 | 0.033 | 1197 (378-1225, n=42); 1163 (964-1217, n=39) |
| glossy-across-4k-cap1-m17f-high | 2/2 | 4.538 | 0.008 | 0.013 | 0.503 | 3.589 | 0.000 | 0.000 | 25.050 | 0.005 | 35.968 | 0.001 | 35.985 | 0.017 | 1143 (276-1220, n=43); 1150 (327-1215, n=41) |
| glossy-across-4k-cap0-m17f-high | 2/2 | 4.533 | 0.002 | 0.014 | 0.501 | 3.594 | 0.000 | 0.000 | 25.045 | 0.010 | 35.957 | 0.052 | 35.991 | 0.035 | 1186 (953-1217, n=40); 1187 (941-1224, n=40) |
| glossy-down-1440-cap2-m17f-high | 2/2 | 3.051 | 0.004 | 0.014 | 0.469 | 1.886 | 0.000 | 0.000 | 10.212 | 0.030 | 16.783 | 0.048 | 16.825 | 0.042 | 1078 (452-1168, n=21); 1095 (964-1149, n=21) |
| glossy-down-1440-cap1-m17f-high | 2/2 | 3.049 | 0.003 | 0.016 | 0.454 | 1.886 | 0.000 | 0.000 | 10.204 | 0.027 | 16.772 | 0.006 | 16.815 | 0.043 | 1091 (722-1158, n=21); 1062 (432-1146, n=21) |
| glossy-down-1440-cap0-m17f-high | 2/2 | 3.050 | 0.000 | 0.015 | 0.433 | 1.885 | 0.000 | 0.000 | 10.223 | 0.219 | 16.756 | 0.033 | 16.796 | 0.041 | 1077 (894-1156, n=21); 1096 (475-1154, n=21) |
| glossy-down-4k-cap2-m17f-high | 2/2 | 6.792 | 0.002 | 0.015 | 0.457 | 4.128 | 0.000 | 0.000 | 23.340 | 0.071 | 36.703 | 0.064 | 36.743 | 0.040 | 1162 (972-1216, n=41); 1175 (956-1216, n=41) |
| glossy-down-4k-cap1-m17f-high | 2/2 | 6.793 | 0.008 | 0.016 | 0.455 | 4.135 | 0.000 | 0.000 | 23.334 | 0.075 | 36.690 | 0.090 | 36.730 | 0.040 | 1162 (937-1222, n=42); 1186 (949-1218, n=41) |
| glossy-down-4k-cap0-m17f-high | 2/2 | 6.806 | 0.014 | 0.016 | 0.456 | 4.127 | 0.000 | 0.000 | 23.348 | 0.035 | 36.718 | 0.008 | 36.756 | 0.038 | 1160 (947-1218, n=41); 1163 (386-1216, n=41) |
| glossy-showcase-hero-1440-cap2-m17f-high | 2/2 | 2.778 | 0.003 | 0.017 | 0.282 | 1.472 | 0.809 | 0.003 | 17.836 | 0.012 | 25.018 | 0.015 | 25.320 | 0.303 | 1087 (853-1186, n=30); 1071 (506-1201, n=31) |
| glossy-showcase-hero-1440-cap1-m17f-high | 2/2 | 2.778 | 0.011 | 0.019 | 0.278 | 1.471 | 0.810 | 0.000 | 17.841 | 0.006 | 25.031 | 0.045 | 25.335 | 0.304 | 1087 (423-1224, n=31); 1121 (461-1211, n=30) |
| glossy-showcase-hero-1440-cap0-m17f-high | 2/2 | 2.781 | 0.001 | 0.017 | 0.286 | 1.472 | 0.809 | 0.005 | 17.838 | 0.014 | 25.016 | 0.021 | 25.316 | 0.300 | 1078 (876-1200, n=29); 1120 (866-1196, n=30) |
| glossy-showcase-hero-4k-cap2-m17f-high | 2/2 | 6.103 | 0.002 | 0.019 | 0.500 | 3.185 | 1.751 | 0.003 | 39.523 | 0.019 | 53.717 | 0.057 | 54.013 | 0.296 | 1210 (431-1226, n=59); 1210 (533-1225, n=60) |
| glossy-showcase-hero-4k-cap1-m17f-high | 2/2 | 6.107 | 0.000 | 0.019 | 0.503 | 3.187 | 1.749 | 0.000 | 39.523 | 0.023 | 53.724 | 0.029 | 54.019 | 0.295 | 1213 (850-1225, n=60); 1208 (919-1225, n=59) |
| glossy-showcase-hero-4k-cap0-m17f-high | 2/2 | 6.104 | 0.002 | 0.019 | 0.504 | 3.188 | 1.749 | 0.003 | 39.546 | 0.008 | 53.745 | 0.013 | 54.047 | 0.302 | 1205 (430-1225, n=60); 1201 (464-1225, n=59) |
