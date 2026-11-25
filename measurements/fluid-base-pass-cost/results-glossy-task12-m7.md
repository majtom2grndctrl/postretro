# Glossy world reflections, 5300M (glossy-task12-m7)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame; chain = every Glossy Floor Chain Level pass in the frame, summed; march = Glossy Floor March Pass; mover = Kinematic Brush Pass. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | chain ms | march ms | mover ms | spread | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-mover-across-1440-after-m17f-high-specpixel | 2/2 | 1.899 | 0.002 | 0.044 | 0.296 | 1.599 | 2.650 | 0.068 | 15.512 | 0.043 | 23.735 | 0.179 | 23.965 | 0.229 | 1078 (928-1199, n=29); 1142 (905-1196, n=29) |
| glossy-mover-across-1440-after-m17f-low-specpixel | 2/2 | 1.818 | 0.002 | 0.042 | 0.000 | 0.000 | 2.607 | 0.024 | 15.092 | 0.029 | 21.267 | 0.035 | 21.487 | 0.219 | 1123 (931-1213, n=26); 1091 (437-1184, n=26) |
| glossy-mover-across-1440-before-m17f-high-specpixel | 2/2 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 2.906 | 0.002 | 15.011 | 0.034 | 19.518 | 0.051 | 19.710 | 0.192 | 1107 (923-1179, n=24); 1094 (915-1188, n=23) |
| glossy-mover-across-1440-after-m17f-high-specsmooth | 2/2 | 1.892 | 0.006 | 0.043 | 0.291 | 1.605 | 2.504 | 0.030 | 15.247 | 0.006 | 23.256 | 0.036 | 23.477 | 0.221 | 1059 (304-1191, n=30); 1119 (816-1211, n=28) |
| glossy-mover-across-1440-after-m17f-low-specsmooth | 2/2 | 1.820 | 0.001 | 0.042 | 0.000 | 0.000 | 2.489 | 0.001 | 14.802 | 0.032 | 20.875 | 0.108 | 21.098 | 0.223 | 1108 (462-1197, n=26); 1127 (476-1224, n=25) |
| glossy-mover-across-1440-before-m17f-high-specsmooth | 2/2 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 2.575 | 0.007 | 14.739 | 0.026 | 18.942 | 0.038 | 19.129 | 0.187 | 1117 (369-1221, n=23); 1119 (746-1177, n=23) |
| glossy-mover-across-4k-after-m17f-high-specpixel | 2/2 | 4.169 | 0.022 | 0.041 | 0.530 | 3.561 | 5.917 | 0.024 | 34.426 | 0.004 | 51.127 | 0.015 | 51.341 | 0.214 | 1206 (466-1226, n=57); 1201 (953-1223, n=56) |
| glossy-mover-across-4k-after-m17f-low-specpixel | 2/2 | 3.966 | 0.033 | 0.040 | 0.000 | 0.000 | 5.875 | 0.057 | 33.539 | 0.009 | 45.876 | 0.006 | 46.082 | 0.206 | 1192 (936-1224, n=50); 1211 (461-1223, n=50) |
| glossy-mover-across-4k-before-m17f-high-specpixel | 2/2 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 6.342 | 0.026 | 33.225 | 0.180 | 42.044 | 0.213 | 42.226 | 0.182 | 1167 (994-1218, n=47); 1182 (1013-1222, n=46) |
| glossy-mover-across-4k-after-m17f-high-specsmooth | 2/2 | 4.168 | 0.008 | 0.039 | 0.531 | 3.570 | 5.602 | 0.034 | 33.805 | 0.026 | 50.237 | 0.015 | 50.459 | 0.221 | 1181 (455-1223, n=56); 1163 (657-1225, n=55) |
| glossy-mover-across-4k-after-m17f-low-specsmooth | 2/2 | 3.979 | 0.000 | 0.046 | 0.000 | 0.000 | 5.608 | 0.020 | 32.942 | 0.003 | 45.040 | 0.009 | 45.252 | 0.212 | 1178 (299-1223, n=51); 1192 (442-1222, n=50) |
| glossy-mover-across-4k-before-m17f-high-specsmooth | 2/2 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 5.614 | 0.002 | 32.604 | 0.038 | 40.685 | 0.022 | 40.868 | 0.183 | 1216 (974-1223, n=45); 1212 (877-1224, n=45) |

## M7: the mover pass with highlight separation, against the build before (Task 12b, HEAD f5b479b44)

Register half: Task 11a's screen (Corrections, `plan.md`): every mover entry unchanged with the highlight-free output, `fs_pixel` 127 and `fs_smooth` 119 VGPRs (cliff at 128). This batch is the cost half.

Fixture: `metal/maps/glossy-floor-mover.map`, the floor-only ceiling fixture plus one kinematic mover, a 96-unit block resting on the glossy floor west of the monument and parked at its first waypoint (`start_on_spawn 0`), on screen in the across view (about 15% of the frame). Pose `glossy-mover-across` (`glossy-across`'s camera). Each binary baked it with its own `prl-build --no-cache` (after: `--glossy-reflections`; 2 probes, 7 world bindings). After binary: release build of `ca71993bf` (HEAD's production source is unchanged since); before: `8dda8a34a`. A fifth arm field sets the capture's `specular_style` (the mover builds one beauty pipeline per style). High runs the floor layer (the mover pass writes the highlight-free target; every High run recorded the chain and the floor march); Low runs the probe term alone (no highlight-free output). 2 interleaved rounds, 300 + 300 frames, mask 17F. 24/24 runs valid.

| resolution | style | before (High) ms | after Low ms | after High ms | High − Low | High − before |
|---|---|---|---|---|---|---|
| 1440p | pixel | 2.906 | 2.607 | 2.650 | +0.043 (+1.6%) | −0.256 (−8.8%) |
| 1440p | smooth | 2.575 | 2.489 | 2.504 | +0.015 (+0.6%) | −0.071 (−2.8%) |
| 4K | pixel | 6.342 | 5.875 | 5.917 | +0.042 (+0.7%) | −0.425 (−6.7%) |
| 4K | smooth | 5.614 | 5.608 | 5.602 | −0.006 (−0.1%) | −0.012 (−0.2%) |

Kinematic Brush Pass per complete frame, mean of 2 runs; every arm's run spread is ≤ 0.034 ms. The highlight-free output costs the mover pass about 0.04 ms in pixel style and nothing measurable in smooth. Against the build before, HEAD's mover pass is cheaper in pixel style (−0.26 / −0.43 ms) and the same in smooth; the branch did not set out to change the mover's pixel path, so the saving's cause (this branch or the `origin/main` merge) is unattributed. The same frames show forward's floor-tier third output again: Textured Pass High − Low +0.42 ms at 1440p and +0.89 ms at 4K in pixel style (checkpoint 4: 0.861 ms at 4K across on the floor-only fixture; ceiling 1.08).

**Kit:** `run_ablation.py` gains the `glossy-mover-across-{auto,1440,4k}` configs, a `mover` column (Kinematic Brush Pass, required in every complete frame on the mover pose) and the optional specular-style arm field; `run.py` reads `XCTRACE_GRACE` (default 180 s), the seconds xctrace may keep saving after the engine's report before it is interrupted. This batch ran with 900. On HEAD's binary at a floor tier with the mover on screen, xctrace's save after the engine exits took longer than 180 s in 2 of 2 pilot attempts (process at 100% CPU, about 2.8 GB resident; at Low and on the before binary it finished in time): the first attempt's figures were complete but flagged, the second was terminated and its export failed. With 900 s every run finished on its own (diagnostic single runs `glossy-task12-m7-probe{,2,3,4}-*` in `metal/runs/`; their results files were dropped). Pilot (`results-glossy-task12-m7-pilot.md`, third attempt, grace 900): 3/3 valid.
