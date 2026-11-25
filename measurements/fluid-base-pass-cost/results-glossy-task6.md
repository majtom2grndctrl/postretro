# Glossy world reflections, 5300M (glossy-task6)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-auto-after-m17f-high | 2/2 | 0.822 | 0.001 | 0.017 | 4.850 | 0.002 | 6.732 | 0.020 | 6.758 | 0.026 | 897 (811-1023, n=10); 897 (334-1078, n=10) |
| glossy-across-1440-after-m17f-high | 2/2 | 1.986 | 0.007 | 0.016 | 11.424 | 0.007 | 14.823 | 0.049 | 14.850 | 0.028 | 1129 (1025-1172, n=18); 1100 (936-1200, n=17) |
| glossy-across-4k-after-m17f-high | 2/2 | 4.348 | 0.009 | 0.015 | 25.489 | 0.023 | 32.082 | 0.035 | 32.125 | 0.043 | 1202 (1022-1219, n=35); 1205 (362-1221, n=36) |
| glossy-down-auto-after-m17f-high | 2/2 | 0.944 | 0.020 | 0.018 | 4.322 | 0.108 | 6.088 | 0.128 | 6.111 | 0.023 | 865 (785-1042, n=9); 898 (262-1002, n=9) |
| glossy-down-1440-after-m17f-high | 2/2 | 2.270 | 0.016 | 0.017 | 10.273 | 0.029 | 13.699 | 0.049 | 13.748 | 0.050 | 1072 (982-1155, n=17); 1048 (916-1150, n=16) |
| glossy-down-4k-after-m17f-high | 2/2 | 5.039 | 0.014 | 0.018 | 23.016 | 0.043 | 30.054 | 0.077 | 30.078 | 0.025 | 1167 (1016-1221, n=33); 1183 (1048-1215, n=34) |
| glossy-across-1440-before-m17f-high | 2/2 | 0.000 | 0.000 | 0.000 | 11.364 | 0.001 | 12.765 | 0.018 | 12.810 | 0.044 | 1051 (321-1198, n=17); 1056 (990-1177, n=15) |
| glossy-across-1440-after-m17f-off | 2/2 | 0.000 | 0.000 | 0.000 | 11.344 | 0.007 | 12.741 | 0.011 | 12.790 | 0.049 | 1037 (360-1152, n=16); 1106 (952-1158, n=15) |
| glossy-across-4k-before-m17f-high | 2/2 | 0.000 | 0.000 | 0.000 | 25.053 | 0.055 | 27.246 | 0.053 | 27.291 | 0.045 | 1179 (1004-1214, n=30); 1188 (1050-1214, n=30) |
| glossy-across-4k-after-m17f-off | 2/2 | 0.000 | 0.000 | 0.000 | 25.054 | 0.004 | 27.249 | 0.021 | 27.290 | 0.042 | 1157 (1010-1211, n=30); 1176 (1027-1213, n=30) |
| near-4k-before-m17f | 2/2 | 0.000 | 0.000 | 0.055 | 31.454 | 0.017 | 60.922 | 0.019 | 61.019 | 0.096 | 1220 (864-1226, n=66); 1220 (1060-1227, n=65) |
| near-4k-after-m17f | 2/2 | 0.000 | 0.000 | 0.034 | 31.471 | 0.026 | 61.037 | 0.050 | 61.109 | 0.073 | 1218 (505-1226, n=65); 1220 (1099-1228, n=65) |

## Task 6 verdict (glossy-world-reflections, after `9dde1debb`)

Binaries: before = `8dda8a34a` (pre-branch, as in glossy-m5m6); after = HEAD `9dde1debb`, `--release --features capture,dev-tools`. Each binary reads maps its own `prl-build --no-cache` wrote (after: `--glossy-reflections`). 2 interleaved rounds, 300 warmup + 300 sampled frames, mask 0x17F, `SKIP_RAW=1`. 42 of 42 main runs valid (this table and the two below). Kit scripts unchanged.

Maps:
- **This table** reads the Task 5 map (`git show 7ea466c0f:content/dev/maps/glossy-reflections-test.map`) baked by the HEAD compiler. HEAD now gives walls probes, so that map's `concrete_carved_tinted_gloss` north wall, which bound no probe at Task 5, now reflects: 4 probes, 18 glossy faces (7 floor, 11 wall). `across` sees that wall (about a quarter of the frame, upper left); `down` does not.
- **Floor-only** (`results-glossy-task6-flooronly.md`): the same Task 5 map with the north wall's inner face retextured to `concrete_stone_021` (a scratch copy), baked by each binary's own compiler. Its glossy coverage is Task 5's: 7 floor faces. Its before arms read that map too, so forward's added cost compares like for like.
- **Current** (`results-glossy-task6-current.md`): the HEAD map, information only.

### Ceilings

| ceiling | Task 5 map, HEAD bake (wall glossy) | floor-only (Task 5 coverage) | ceiling | verdict |
|---|---|---|---|---|
| overlay across Auto | 0.822 | 0.607 | 0.76 | BREACH on the Task 5 map; PASS floor-only |
| overlay across 1440p | 1.986 | 1.432 | 1.83 | BREACH on the Task 5 map; PASS floor-only |
| overlay across 4K | 4.348 | 3.214 | 4.07 | BREACH on the Task 5 map; PASS floor-only |
| overlay down Auto | 0.944 | (same pixels) | 1.43 | PASS |
| overlay down 1440p | 2.270 | (same pixels) | 3.16 | PASS |
| overlay down 4K | 5.039 | (same pixels) | 7.05 | PASS |
| forward added, 4K across, High vs before | 25.489 - 25.053 = 0.436 | 24.510 - 24.220 = 0.290 | 0.41 | BREACH on the Task 5 map; PASS floor-only |
| probe sums | 0.015-0.018 | 0.015-0.017 | 0.025 | PASS |
| forward at Off vs before High, across | 1440p 11.344 vs 11.364; 4K 25.054 vs 25.053 | | unchanged | PASS; PNGs byte-identical at 1440p and 4K |
| forward on e24 near 4K | 31.471 vs 31.454 (spreads 0.026, 0.017) | | unchanged | PASS; PNGs byte-identical |

Every across breach is coverage, not per-pixel cost. On Task 5's glossy pixels the overlay matches Task 5's measurement (0.608 / 1.464 / 3.258 ms then, 0.607 / 1.432 / 3.214 now), and the glossy north wall adds 1.13 ms at 4K. Forward's added cost at 4K across, 0.29 ms on floor-only, is no more than the 0.33 ms measured with Rg11b10Ufloat; the wider Rgba16Float D_static shows no cost there. The down overlay fell 11% (5.637 to 5.039 ms at 4K) with the same pixels, consistent with zero-weight texels skipping the probe search.

### Current map (information; new baseline)

| arm | overlay ms | sums ms | forward ms | GPU frame ms |
|---|---|---|---|---|
| across 1440p | 1.885 | 0.020 | 11.641 | 16.171 |
| across 4K | 4.140 | 0.019 | 25.862 | 33.436 |
| down 1440p | 1.892 | 0.021 | 10.613 | 14.916 |
| down 4K | 4.200 | 0.022 | 23.785 | 31.204 |

`across` sees the glossy floor (about half the frame), the glossy north wall (upper left), a strip of the glossy ceiling along the top edge and the end of the glossy ramp at the left edge; the non-glossy floor panel hides some foreground floor. `down` sees floor, with the non-glossy panel covering about a quarter of the frame (upper left), so it has fewer glossy pixels than on the Task 5 map (4.20 vs 5.04 ms at 4K). The panel's animated light adds passes the Task 5 map lacks (4K across: Streamed Animated Direct SH 0.44, Animated LM Compose 0.37, Streamed SH Compose 0.34 ms), so frame time is not comparable with the Task 5 map.

### Runs

- Pilots, one run per arm: `results-glossy-task6-pilot.md` (stopped at its first invalid run, `glossy-down-auto` r1: complete-frame 6.49 ms vs incomplete 6.86 ms, dropped rows), `results-glossy-task6-pilot2.md` (the rest, `glossy-down-auto` valid on retry), `results-glossy-task6-flooronly-pilot.md` and `results-glossy-task6-current-pilot.md`. All other pilot runs were valid and agree with the main runs.
- Clocks: busy-sample medians 1037-1205 MHz at 1440p and 4K, 865-921 MHz at Auto. No run sat near 85 MHz. Single low samples (262-374 MHz) appear only as run minima. Auto arms are clock-unstable as before; read them as flagged.
