# Glossy world reflections, 5300M (glossy-t256-experiment)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame; chain = every Glossy Floor Chain Level pass in the frame, summed; march = Glossy Floor March Pass; mover = Kinematic Brush Pass. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | chain ms | march ms | mover ms | spread | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-showcase-hero128-1440-cap1-m17f-high | 2/2 | 2.778 | 0.001 | 0.027 | 0.351 | 1.482 | 0.219 | 0.004 | 17.864 | 0.027 | 25.128 | 0.023 | 25.443 | 0.315 | 1042 (530-1203, n=32); 1078 (909-1188, n=30) |
| glossy-showcase-hero-1440-cap0-m17f-high | 2/2 | 2.792 | 0.003 | 0.025 | 0.311 | 1.478 | 0.305 | 0.143 | 17.836 | 0.004 | 25.082 | 0.040 | 25.394 | 0.313 | 1112 (452-1192, n=31); 1053 (880-1185, n=30) |
