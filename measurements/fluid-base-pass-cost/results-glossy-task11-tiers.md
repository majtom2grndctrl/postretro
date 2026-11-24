# Glossy world reflections, 5300M (glossy-task11-tiers)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame; chain = every Glossy Floor Chain Level pass in the frame, summed; march = Glossy Floor March Pass. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | chain ms | march ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-auto-after-m17f-medium | 1/2 | 0.868 | n/a | 0.015 | 0.281 | 0.197 | 4.926 | n/a | 7.356 | n/a | 7.386 | 0.030 | 837 (672-1035, n=11); 924 (403-1061, n=12) |
| glossy-across-1440-after-m17f-medium | 2/2 | 2.047 | 0.004 | 0.016 | 0.860 | 0.393 | 10.548 | 0.039 | 15.318 | 0.025 | 15.363 | 0.045 | 1041 (621-1133, n=19); 1052 (879-1156, n=19) |
| glossy-across-4k-after-m17f-medium | 2/2 | 4.527 | 0.005 | 0.015 | 0.394 | 0.769 | 24.912 | 0.253 | 32.874 | 0.011 | 32.933 | 0.059 | 1174 (949-1211, n=38); 1147 (954-1213, n=37) |
| glossy-down-auto-after-m17f-medium | 1/2 | 1.473 | n/a | 0.020 | 1.497 | 0.240 | 3.659 | n/a | 7.715 | n/a | 7.754 | 0.039 | 838 (362-968, n=11); 845 (683-1032, n=11) |
| glossy-down-1440-after-m17f-medium | 2/2 | 3.052 | 0.007 | 0.018 | 3.047 | 0.458 | 7.555 | 0.079 | 15.287 | 0.029 | 15.338 | 0.050 | 1098 (500-1201, n=19); 1040 (871-1128, n=19) |
| glossy-down-4k-after-m17f-medium | 2/2 | 6.763 | 0.009 | 0.017 | 1.224 | 0.933 | 22.311 | 0.998 | 33.233 | 0.127 | 33.284 | 0.051 | 1141 (948-1215, n=38); 1160 (968-1218, n=37) |
| glossy-across-auto-after-m17f-very_high | 2/2 | 0.832 | 0.004 | 0.016 | 0.227 | 1.216 | 4.864 | 0.066 | 8.215 | 0.033 | 8.238 | 0.023 | 877 (785-1044, n=12); 925 (366-991, n=13) |
| glossy-across-1440-after-m17f-very_high | 2/2 | 2.046 | 0.000 | 0.015 | 0.278 | 2.858 | 11.220 | 0.032 | 17.871 | 0.011 | 17.920 | 0.049 | 1063 (879-1144, n=22); 1106 (274-1178, n=23) |
| glossy-across-4k-after-m17f-very_high | 2/2 | 4.496 | 0.002 | 0.015 | 0.456 | 6.564 | 25.060 | 0.041 | 38.849 | 0.050 | 38.893 | 0.044 | 1162 (1056-1220, n=43); 1173 (933-1224, n=43) |
| glossy-down-auto-after-m17f-very_high | 2/2 | 1.314 | 0.028 | 0.018 | 0.289 | 1.437 | 4.393 | 0.255 | 8.233 | 0.261 | 8.268 | 0.035 | 903 (277-1030, n=13); 873 (727-1018, n=12) |
| glossy-down-1440-after-m17f-very_high | 2/2 | 3.050 | 0.002 | 0.016 | 1.354 | 3.306 | 9.334 | 0.196 | 18.214 | 0.025 | 18.276 | 0.062 | 1104 (917-1170, n=22); 1107 (931-1159, n=22) |
| glossy-down-4k-after-m17f-very_high | 2/2 | 6.774 | 0.001 | 0.017 | 0.425 | 7.344 | 23.326 | 0.001 | 39.838 | 0.012 | 39.886 | 0.048 | 1167 (994-1219, n=44); 1171 (968-1219, n=44) |
| glossy-across-auto-after-m17f-ultra | 2/2 | 0.838 | 0.013 | 0.016 | 0.179 | 1.740 | 4.936 | 0.101 | 8.761 | 0.123 | 8.797 | 0.036 | 802 (719-1000, n=12); 890 (335-1014, n=13) |
| glossy-across-1440-after-m17f-ultra | 2/2 | 2.043 | 0.001 | 0.015 | 0.382 | 4.061 | 11.116 | 0.065 | 19.046 | 0.026 | 19.095 | 0.049 | 1051 (895-1174, n=23); 1115 (946-1206, n=23) |
| glossy-across-4k-after-m17f-ultra | 2/2 | 4.496 | 0.005 | 0.014 | 0.457 | 9.324 | 25.031 | 0.020 | 41.584 | 0.002 | 41.627 | 0.043 | 1172 (871-1219, n=47); 1193 (995-1225, n=45) |
| glossy-down-auto-after-m17f-ultra | 0/2 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | 905 (287-1007, n=14); 892 (596-1150, n=14) |
| glossy-down-1440-after-m17f-ultra | 2/2 | 3.058 | 0.005 | 0.016 | 1.255 | 4.686 | 9.465 | 0.220 | 19.642 | 0.066 | 19.692 | 0.050 | 1104 (297-1161, n=25); 1044 (900-1168, n=24) |
| glossy-down-4k-after-m17f-ultra | 2/2 | 6.777 | 0.032 | 0.016 | 0.428 | 10.444 | 23.409 | 0.097 | 43.048 | 0.159 | 43.093 | 0.045 | 1181 (939-1222, n=47); 1185 (969-1220, n=48) |

Invalid runs:
- glossy-task11-tiers-glossy-down-auto-after-m17f-medium-r1: GPU frame time differs between complete (7.2705 ms) and incomplete (7.897 ms) frames: dropped rows lost time, not only labels
- glossy-task11-tiers-glossy-down-auto-after-m17f-ultra-r1: GPU frame time differs between complete (8.9551 ms) and incomplete (9.2306 ms) frames: dropped rows lost time, not only labels
- glossy-task11-tiers-glossy-down-auto-after-m17f-ultra-r2: GPU frame time differs between complete (9.4835 ms) and incomplete (10.1051 ms) frames: dropped rows lost time, not only labels
- glossy-task11-tiers-glossy-across-auto-after-m17f-medium-r2: GPU frame time differs between complete (7.2371 ms) and incomplete (7.5696 ms) frames: dropped rows lost time, not only labels

## Floor layer per tier (Task 11, HEAD ca71993bf, checkpoint 4)

Same binary and `.prl` as `results-glossy-task11-flooronly.md` (floor-only fixture, mask 17F, 2 interleaved rounds, 300 + 300 frames). 32/36 runs valid; all four invalid runs are Auto (complete and incomplete frame times differ by more than 3%). Auto figures prefer the Auto batch (`results-glossy-task11-auto.md`, 16/20 valid; Auto Medium across had 0/2 valid there, so this batch's single valid run stands). High comes from the floor-only batch. Presets (`FLOOR_LAYER_PRESETS`, target fraction / steps / divisor): medium ¼ / 48 / 180, high ½ / 90 / 180, very high ½ / 180 / 360, ultra ½ / 270 / 540.

- **chain**: every Glossy Floor Chain Level pass in the frame, summed (10–12 levels).
- **march**: the Glossy Floor March Pass.
- **forward + chain**: the Textured Pass plus the chain. The trace moves part of the Textured Pass's time into the first chain level by an amount that varies with pose and tier (`results-glossy-task11-diag.md`), so only the sum is stable.
- **floor layer added**: forward + chain + march at the tier, minus forward at Low on the same binary and fixture (from the floor-only and Auto batches). That is the floor layer's whole cost outside the overlay: highlight-free output, chain and march.
- **Proposed ceilings**: measured + 25% (Delegated answers), per tier, pose and resolution. The overlay's own ceilings stay as they are; the overlay costs the same at every floor tier (see the table above).

| tier | pose | res | valid (batch) | chain | march | forward + chain | floor layer added | proposed march ceiling | proposed floor-layer ceiling |
|---|---|---|---|---|---|---|---|---|---|
| medium | across | Auto | 1/2 (tiers) | 0.281 | 0.197 | 5.207 | 0.646 | 0.25 | 0.81 |
| medium | across | 1440p | 2/2 (tiers) | 0.860 | 0.393 | 11.409 | 0.839 | 0.49 | 1.05 |
| medium | across | 4K | 2/2 (tiers) | 0.394 | 0.769 | 25.306 | 1.575 | 0.96 | 1.97 |
| medium | down | Auto | 1/2 (auto) | 1.058 | 0.216 | 4.698 | 0.482 | 0.27 | 0.60 |
| medium | down | 1440p | 2/2 (tiers) | 3.047 | 0.458 | 10.602 | 0.803 | 0.57 | 1.00 |
| medium | down | 4K | 2/2 (tiers) | 1.224 | 0.933 | 23.535 | 1.525 | 1.17 | 1.91 |
| high | across | Auto | 2/2 (auto) | 0.172 | 0.653 | 5.056 | 0.951 | 0.82 | 1.19 |
| high | across | 1440p | 2/2 (flooronly) | 0.395 | 1.612 | 11.522 | 2.172 | 2.01 | 2.71 |
| high | across | 4K | 2/2 (flooronly) | 0.465 | 3.618 | 25.541 | 4.659 | 4.52 | 5.82 |
| high | down | Auto | 2/2 (auto) | 0.260 | 0.819 | 4.806 | 1.193 | 1.02 | 1.49 |
| high | down | 1440p | 2/2 (flooronly) | 1.133 | 1.903 | 10.664 | 2.311 | 2.38 | 2.89 |
| high | down | 4K | 2/2 (flooronly) | 0.436 | 4.139 | 23.770 | 4.966 | 5.17 | 6.21 |
| very high | across | Auto | 2/2 (auto) | 0.225 | 1.216 | 5.098 | 1.557 | 1.52 | 1.95 |
| very high | across | 1440p | 2/2 (tiers) | 0.278 | 2.858 | 11.498 | 3.394 | 3.57 | 4.24 |
| very high | across | 4K | 2/2 (tiers) | 0.456 | 6.564 | 25.516 | 7.580 | 8.21 | 9.48 |
| very high | down | Auto | 1/2 (auto) | 0.487 | 1.398 | 4.598 | 1.564 | 1.75 | 1.95 |
| very high | down | 1440p | 2/2 (tiers) | 1.354 | 3.306 | 10.688 | 3.738 | 4.13 | 4.67 |
| very high | down | 4K | 2/2 (tiers) | 0.425 | 7.344 | 23.751 | 8.151 | 9.18 | 10.19 |
| ultra | across | Auto | 2/2 (auto) | 0.261 | 1.741 | 5.121 | 2.106 | 2.18 | 2.63 |
| ultra | across | 1440p | 2/2 (tiers) | 0.382 | 4.061 | 11.498 | 4.597 | 5.08 | 5.75 |
| ultra | across | 4K | 2/2 (tiers) | 0.457 | 9.324 | 25.488 | 10.312 | 11.66 | 12.89 |
| ultra | down | Auto | 2/2 (auto) | 0.311 | 2.055 | 4.738 | 2.361 | 2.57 | 2.95 |
| ultra | down | 1440p | 2/2 (tiers) | 1.255 | 4.686 | 10.720 | 5.150 | 5.86 | 6.44 |
| ultra | down | 4K | 2/2 (tiers) | 0.428 | 10.444 | 23.837 | 11.337 | 13.06 | 14.17 |

Reading the table:
- **The march dominates and scales with the preset.** At High it is 1.6 / 3.6 ms across and 1.9 / 4.1 ms down (1440p / 4K). Very high is about 1.8× High, Ultra about 2.5× (steps 2× and 3×). Medium (¼ target, 48 steps) is about a quarter of High.
- **The chain is cheap and tier-independent.** Levels 1 and up take 0.17–0.22 ms in total; level 0 is at least 0.16 / 0.24 ms at 1440p / 4K. The chain column's larger values (down 1440p, every Medium arm, down 4K Medium) are forward's time moved into level 0, not chain work. Forward + chain holds within 0.1–0.3 ms across tiers at each pose and resolution.
- **A floor-tier frame at 4K is heavy.** On this fixture the floor layer adds about 4.7–5.0 ms at High, 7.6–8.2 ms at Very high and 10.3–11.3 ms at Ultra, on top of the overlay (4.5 / 6.8 ms).
- **Suggested tripwire form.** The march column per tier is the sharpest test (spread at most 0.046 ms between runs at 1440p and 4K). The floor-layer-added column also covers the highlight-free output and the chain, but it compares forward across two batches, so allow about 0.1 ms of noise.
- **Auto is clock-unstable,** as in every earlier check (busy medians 611–1007 MHz), and about one Auto run in four fails the complete-frame check. Treat Auto as indicative; 1440p and 4K are the reliable figures.
