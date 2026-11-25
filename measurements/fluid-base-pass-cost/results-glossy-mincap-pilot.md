# Glossy world reflections, 5300M (glossy-mincap-pilot)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame; chain = every Glossy Floor Chain Level pass in the frame, summed; march = Glossy Floor March Pass; mover = Kinematic Brush Pass. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | chain ms | march ms | mover ms | spread | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-4k-cap2-m17f-high | 1/1 | 4.526 | n/a | 0.013 | 0.504 | 3.600 | 0.000 | n/a | 25.061 | n/a | 35.969 | n/a | 36.002 | 0.033 | 1186 (949-1217, n=40) |
| glossy-down-1440-cap1-m17f-high | 1/1 | 3.052 | n/a | 0.015 | 0.430 | 1.887 | 0.000 | n/a | 10.229 | n/a | 16.766 | n/a | 16.805 | 0.040 | 1049 (444-1152, n=21) |
| glossy-across-4k-cap0-m17f-high | 1/1 | 4.532 | n/a | 0.014 | 0.502 | 3.599 | 0.000 | n/a | 25.062 | n/a | 35.992 | n/a | 35.995 | 0.003 | 1167 (508-1218, n=41) |
| glossy-showcase-hero-4k-cap0-m17f-high | 1/1 | 6.104 | n/a | 0.022 | 0.500 | 3.187 | 1.749 | n/a | 39.572 | n/a | 53.773 | n/a | 54.066 | 0.293 | 1207 (630-1226, n=60) |
