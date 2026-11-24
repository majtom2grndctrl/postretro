# Glossy world reflections, 5300M (glossy-task11-flooronly)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame; chain = every Glossy Floor Chain Level pass in the frame, summed; march = Glossy Floor March Pass. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | chain ms | march ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-auto-after-m17f-high | 1/2 | 0.837 | n/a | 0.015 | 0.214 | 0.662 | 4.905 | n/a | 7.677 | n/a | 7.710 | 0.033 | 909 (788-1026, n=11); 835 (685-992, n=12) |
| glossy-across-1440-after-m17f-high | 2/2 | 2.049 | 0.003 | 0.015 | 0.395 | 1.612 | 11.127 | 0.099 | 16.632 | 0.055 | 16.702 | 0.069 | 1051 (888-1171, n=21); 1110 (915-1154, n=20) |
| glossy-across-4k-after-m17f-high | 2/2 | 4.506 | 0.021 | 0.014 | 0.465 | 3.618 | 25.077 | 0.007 | 35.953 | 0.012 | 35.995 | 0.043 | 1181 (978-1217, n=40); 1157 (960-1218, n=39) |
| glossy-down-auto-after-m17f-high | 1/2 | 1.374 | n/a | 0.018 | 0.396 | 0.835 | 4.495 | n/a | 7.908 | n/a | 7.943 | 0.035 | 809 (540-975, n=12); 843 (256-1148, n=13) |
| glossy-down-1440-after-m17f-high | 2/2 | 3.049 | 0.007 | 0.017 | 1.133 | 1.903 | 9.531 | 0.133 | 16.797 | 0.005 | 16.849 | 0.052 | 1086 (900-1150, n=21); 1063 (878-1151, n=21) |
| glossy-down-4k-after-m17f-high | 2/2 | 6.764 | 0.010 | 0.016 | 0.436 | 4.139 | 23.334 | 0.024 | 36.636 | 0.029 | 36.683 | 0.047 | 1136 (965-1216, n=41); 1168 (956-1219, n=41) |
| glossy-across-1440-before-m17f-high | 2/2 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 10.927 | 0.008 | 12.414 | 0.038 | 12.438 | 0.024 | 1080 (958-1126, n=15); 1093 (966-1178, n=14) |
| glossy-across-4k-before-m17f-high | 2/2 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 24.216 | 0.029 | 26.473 | 0.033 | 26.514 | 0.041 | 1184 (1038-1211, n=29); 1165 (1035-1217, n=29) |
| glossy-across-1440-after-m17f-low | 2/2 | 1.949 | 0.003 | 0.015 | 0.000 | 0.000 | 10.962 | 0.018 | 14.389 | 0.015 | 14.416 | 0.027 | 1111 (980-1159, n=17); 1113 (964-1164, n=17) |
| glossy-across-4k-after-m17f-low | 2/2 | 4.326 | 0.013 | 0.014 | 0.000 | 0.000 | 24.500 | 0.038 | 31.137 | 0.012 | 31.181 | 0.044 | 1192 (1097-1220, n=34); 1192 (1034-1217, n=34) |
| glossy-down-1440-after-m17f-low | 2/2 | 2.904 | 0.004 | 0.018 | 0.000 | 0.000 | 10.256 | 0.019 | 14.338 | 0.049 | 14.390 | 0.052 | 1082 (958-1171, n=18); 1127 (983-1144, n=17) |
| glossy-down-4k-after-m17f-low | 2/2 | 6.444 | 0.011 | 0.018 | 0.000 | 0.000 | 22.943 | 0.042 | 31.348 | 0.044 | 31.397 | 0.048 | 1170 (1018-1213, n=35); 1177 (1016-1216, n=35) |

Invalid runs:
- glossy-task11-flooronly-glossy-down-auto-after-m17f-high-r2: GPU frame time differs between complete (7.8785 ms) and incomplete (8.3524 ms) frames: dropped rows lost time, not only labels
- glossy-task11-flooronly-glossy-across-auto-after-m17f-high-r2: GPU frame time differs between complete (8.388 ms) and incomplete (8.7121 ms) frames: dropped rows lost time, not only labels

## Ceiling check (Task 11, HEAD ca71993bf, checkpoint 4)

After binary: release build of ca71993bf, `capture,dev-tools`. Before binary: the pre-branch build (8dda8a34a) every earlier check used. Each binary baked `metal/maps/glossy-floor-only.map` with its own `prl-build --no-cache` (after: `--glossy-reflections`). 2 interleaved rounds, 300 + 300 frames, mask 17F. Quality high unless the arm says low. 22/24 runs valid: both invalid runs are Auto (complete and incomplete frame times differ by more than 3%), so the Auto figures below come from a separate Auto batch (`results-glossy-task11-auto.md`, same day, same binaries and `.prl`). Pilot (`results-glossy-task11-flooronly-pilot.md`, one run per config type, abort on the first invalid run): 8/8 valid.

| ceiling | measured | ceiling | margin | result | spike re-baseline | change |
|---|---|---|---|---|---|---|
| overlay across, Auto | 0.831 (Auto batch, 2/2; this batch 0.837, 1/2) | 0.81 | +2.6% over | FAIL | 0.784 | +0.047 |
| overlay across, 1440p | 2.049 | 1.96 | +4.5% over | FAIL | 1.946 | +0.103 |
| overlay across, 4K | 4.506 | 4.34 | +3.8% over | FAIL | 4.317 | +0.189 |
| overlay down, Auto | 1.357 (Auto batch, 2/2) | 1.43 | 5.1% under | PASS | 1.236 | +0.121 |
| overlay down, 1440p | 3.049 | 3.16 | 3.5% under | PASS | 2.907 | +0.142 |
| overlay down, 4K | 6.764 | 7.05 | 4.1% under | PASS | 6.453 | +0.311 |
| forward added, 4K across (25.077 - 24.216) | 0.861 | 0.41 | +110% over | FAIL | 0.342 | +0.519 |
| sums pass (worst arm) | 0.018 | 0.025 | 28% under | PASS | 0.016 | +0.002 |

Forward added compares the glossy level on HEAD at high (`glossy-across-4k-after-m17f-high`) with the pre-branch binary on its own bake of the same fixture (`glossy-across-4k-before-m17f-high`), as earlier checks did. At 1440p across: 11.127 - 10.927 = 0.200 ms.

The pre-branch arms match earlier sessions (forward 10.927 / 24.216 ms at 1440p / 4K, Task 10 10.951 / 24.149), and the Low arms reproduce the spike's probe-term overlay (1.949 / 4.326 ms against 1.946 / 4.317), so the machine is in the same state and the rises are Task 11's.

## Attribution (same session, same `.prl`)

**The layered composite's share of the overlay** (High minus Low; Low draws the probe term alone through the six earlier entries, High the `_floor` entries):

| pose | Auto | 1440p | 4K |
|---|---|---|---|
| across, Low | 0.805 | 1.949 | 4.326 |
| across, High | 0.831 | 2.049 | 4.506 |
| across, composite | +0.026 (3.1%) | +0.100 (4.9%) | +0.180 (4.0%) |
| down, Low | 1.280 | 2.904 | 6.444 |
| down, High | 1.357 | 3.049 | 6.764 |
| down, composite | +0.077 (5.6%) | +0.145 (4.7%) | +0.320 (4.7%) |

The probe term alone stays under every across ceiling, by 0.6% at 1440p and 0.3% at 4K, so the composite's 4–5% alone trips them, as the spike ruling expected of the next per-pixel addition. The overlay costs the same at every floor tier (`results-glossy-task11-tiers.md`: across 2.04–2.05 / 4.50–4.53 ms, down 3.05–3.06 / 6.76–6.78 ms), so the composite's cost does not depend on the tier's preset.

**Forward's highlight-free output.** Forward at Low against the pre-branch binary (the second output alone, as before): +0.035 / +0.284 ms at 1440p / 4K across (spike 0.342 at 4K). Forward at High against Low (the third output): across +0.165 / +0.577 ms, down -0.726 / +0.391 ms. The negative figure is a timing artefact, not a saving. On frames that run the floor layer, the trace's Textured Pass interval ends early, and the rest of its time shows up in the first chain level's interval (`results-glossy-task11-diag.md`). So forward alone understates the third output, and the chain overstates itself, by an amount that varies with pose and tier. Their sum is stable: forward + chain at High minus forward at Low is +0.560 / +1.041 ms across and +0.408 / +0.827 ms down (1440p / 4K). That sum includes the chain's own work: levels 1 and up take 0.17–0.22 ms in total, and level 0 takes at least 0.16 / 0.24 ms (the lowest seen). The third output therefore costs about 0.2–0.4 ms at 1440p and 0.35–0.8 ms at 4K. At 4K across, the forward ceiling's 0.41 ms is exceeded by forward alone even before any chain share is counted.

**Whole frame.** GPU frame at High minus Low on this fixture: across 0.946 / 2.243 / 4.816 ms, down 1.263 / 2.459 / 5.288 ms (Auto / 1440p / 4K). That is the floor layer's total at High, overlay composite included.

## What the fixture cannot show

- **The mover pass's highlight output.** The fixture has no movers, so the mover beauty pass (`@location(1)` highlight-free colour) never records. Its cost on the 5300M is unmeasured here; the register screen is its only check (every mover entry unchanged).
- **The skinned-mesh pass's write.** The fixture has no skinned meshes either.
- **`_s` 0 texels.** The fixture's floor (`test/metal_smoothness_ramp`) has `_s` 255 on every texel, so skipping the march on `_s` 0 texels would save nothing here; its saving depends on authored masks (the dev map's tile-and-grout room).
