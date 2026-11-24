# Glossy world reflections, 5300M (glossy-task12-m6)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame; chain = every Glossy Floor Chain Level pass in the frame, summed; march = Glossy Floor March Pass; mover = Kinematic Brush Pass. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | chain ms | march ms | mover ms | spread | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| near-1440-before-m17f | 2/2 | 0.000 | 0.000 | 0.046 | 0.000 | 0.000 | 0.000 | 0.000 | 14.056 | 0.204 | 27.683 | 0.170 | 27.769 | 0.086 | 1159 (378-1209, n=33); 1174 (996-1202, n=31) |
| near-1440-after-m17f | 2/2 | 0.000 | 0.000 | 0.034 | 0.000 | 0.000 | 0.000 | 0.000 | 14.210 | 0.053 | 28.100 | 0.092 | 28.170 | 0.070 | 1151 (1024-1219, n=32); 1162 (1031-1210, n=32) |
| near-4k-before-m17f | 2/2 | 0.000 | 0.000 | 0.060 | 0.000 | 0.000 | 0.000 | 0.000 | 31.212 | 0.209 | 60.724 | 0.191 | 60.816 | 0.092 | 1218 (1045-1228, n=66); 1218 (990-1225, n=65) |
| near-4k-after-m17f | 2/2 | 0.000 | 0.000 | 0.033 | 0.000 | 0.000 | 0.000 | 0.000 | 31.369 | 0.085 | 61.469 | 0.213 | 61.531 | 0.061 | 1221 (1031-1228, n=66); 1213 (1039-1221, n=65) |
| glossy-across-1440-before-m17f-high | 2/2 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 11.465 | 0.000 | 13.735 | 0.005 | 13.858 | 0.123 | 1074 (336-1140, n=18); 1097 (311-1148, n=17) |
| glossy-across-1440-after-m17f-off | 2/2 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 11.460 | 0.000 | 13.719 | 0.004 | 13.854 | 0.136 | 1056 (312-1153, n=18); 1089 (939-1140, n=16) |
| glossy-across-4k-before-m17f-high | 2/2 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 25.301 | 0.017 | 28.388 | 0.008 | 28.608 | 0.220 | 1185 (996-1215, n=31); 1166 (301-1212, n=32) |
| glossy-across-4k-after-m17f-off | 2/2 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 25.322 | 0.001 | 28.428 | 0.021 | 28.666 | 0.237 | 1168 (1007-1224, n=31); 1188 (382-1216, n=31) |

## M6: forward pass cost against the build before (Task 12b, HEAD f5b479b44)

After binary: release build of `ca71993bf` (`capture,dev-tools`); HEAD's production source is unchanged since (`git diff --stat ca71993bf HEAD -- crates/ ':!*test*'` lists only a `#[cfg(test)]` module registration). Before binary: the pre-branch build of `8dda8a34a`. Each binary reads its own fresh `prl-build --no-cache` bake (after: `--glossy-reflections`), from `MAP_ROOT_BEFORE` / `MAP_ROOT_AFTER` copies of `content/dev`. 2 interleaved rounds, 300 + 300 frames, mask 17F. 16/16 runs valid. Pilot (`results-glossy-task12-m6-pilot.md`, one run per config type, abort on the first invalid run): 4/4 valid.

- **Map without glossy faces:** e24's `near` pose (Task 5's view, the still pool straight down), quality high (default).
- **Glossy fixture at reflection quality off:** `content/dev/maps/glossy-reflections-test.map` at the `glossy-across` pose. After at off (forward's one-output form, no overlay); before at high, which on the before binary draws the same thing (no glossy code, no water on the map).

| view | resolution | forward before ms | forward after ms | Δ ms | Δ % | before run spread | after run spread | result |
|---|---|---|---|---|---|---|---|---|
| e24 near | 1440p | 14.056 | 14.210 | +0.154 | +1.1% | 0.204 | 0.053 | unchanged within spread |
| e24 near | 4K | 31.212 | 31.369 | +0.157 | +0.5% | 0.209 | 0.085 | unchanged within spread |
| glossy-across, off | 1440p | 11.465 | 11.460 | −0.005 | −0.04% | 0.000 | 0.000 | unchanged |
| glossy-across, off | 4K | 25.301 | 25.322 | +0.021 | +0.08% | 0.017 | 0.001 | unchanged |

Forward on e24 moves by less than the before arm's own run-to-run spread at both resolutions (4K pairs, pilot included: −0.02, +0.22, +0.10 ms). At off on the glossy fixture forward is unchanged to 0.1%. Forward's added cost on the glossy fixture at floor and probe tiers (second output and highlight separation) is checkpoint 4's record (`results-glossy-task11-flooronly.md`): 0.861 ms at 4K across at High against the 1.08 ms floor-tier ceiling, 0.29–0.34 ms at Low.

**Outside M6's forward clause, reported:** on e24 the Fluid Surface Pass is slower on HEAD in every run: 5.859 → 6.087 ms at 1440p (+0.228, +3.9%) and 12.869 → 13.421 ms at 4K (+0.552, +4.3%), run spreads ≤ 0.035 ms; every pair, the pilot's included (3 at 4K, 2 at 1440p). Water's pixels are byte-identical to the build before (`results-glossy-task12-parity.md`, row 2), so this is instruction or binding cost in the fluid shader, not output. Candidate causes, unmeasured: Task 10's generic hemisphere-axis search (water passes a constant +Y axis and relies on constant folding), Task 3's six-lobe sums (1536 vec4s, was 1280), and the probe arrays' layout move (Task 4). The probe sums pass is faster on HEAD (0.046 → 0.034 ms at 1440p). GPU frame on e24: +0.42 ms at 1440p, +0.75 ms at 4K.
