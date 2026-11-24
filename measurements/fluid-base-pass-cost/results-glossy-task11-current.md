# Glossy world reflections, 5300M (glossy-task11-current)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame; chain = every Glossy Floor Chain Level pass in the frame, summed; march = Glossy Floor March Pass. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | chain ms | march ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-1440-after-m17f-high | 2/2 | 3.018 | 0.006 | 0.025 | 0.486 | 1.556 | 11.595 | 0.185 | 18.911 | 0.058 | 19.078 | 0.167 | 1096 (908-1206, n=23); 1112 (758-1193, n=23) |
| glossy-across-4k-after-m17f-high | 2/2 | 6.528 | 0.002 | 0.026 | 0.438 | 3.421 | 26.105 | 0.006 | 39.516 | 0.013 | 39.765 | 0.249 | 1194 (566-1226, n=46); 1195 (395-1217, n=45) |
| glossy-down-1440-after-m17f-high | 2/2 | 2.362 | 0.007 | 0.028 | 0.553 | 1.604 | 10.505 | 0.076 | 17.169 | 0.150 | 17.327 | 0.158 | 1081 (878-1199, n=22); 1063 (878-1204, n=22) |
| glossy-down-4k-after-m17f-high | 2/2 | 5.230 | 0.013 | 0.029 | 0.408 | 3.473 | 24.198 | 0.001 | 36.279 | 0.049 | 36.507 | 0.228 | 1192 (945-1218, n=40); 1195 (420-1219, n=41) |

## Current dev map baseline (Task 11, HEAD ca71993bf)

`content/dev/maps/glossy-reflections-test.map` (unchanged since the spike's baseline), baked by HEAD's `prl-build --no-cache --glossy-reflections`. Quality high (a floor tier), mask 17F, 2 interleaved rounds, 300 + 300 frames, 8/8 valid. Pilot (`results-glossy-task11-current-pilot.md`): 1/1 valid. This is a reported baseline, not a ceiling (owner ruling, checkpoint 2).

| pose | res | overlay | chain | march | forward | GPU frame | overlay, spike baseline |
|---|---|---|---|---|---|---|---|
| across | 1440p | 3.018 | 0.486 | 1.556 | 11.595 | 18.911 | 2.909 |
| across | 4K | 6.528 | 0.438 | 3.421 | 26.105 | 39.516 | 6.362 |
| down | 1440p | 2.362 | 0.553 | 1.604 | 10.505 | 17.169 | 2.253 |
| down | 4K | 5.230 | 0.408 | 3.473 | 24.198 | 36.279 | 5.036 |

- The overlay rose by +0.109 / +0.166 ms across and +0.109 / +0.194 ms down (1440p / 4K), +3.7–4.8%, the same share the composite takes on the floor-only fixture.
- The march costs a little less than on the floor-only fixture at the same poses (across 1.556 / 3.421 against 1.612 / 3.618), because fewer of this map's on-screen pixels are up-facing glossy floor.
- Forward here also includes the highlight-free output; see the floor-only doc on why forward and chain are read together.
- The probe sums pass reads 0.025–0.029 ms on this map (spike baseline 0.019–0.022). The sums ceiling (0.025 ms) is set on the floor-only fixture, where it reads 0.014–0.018. This map's figure is a baseline only.
