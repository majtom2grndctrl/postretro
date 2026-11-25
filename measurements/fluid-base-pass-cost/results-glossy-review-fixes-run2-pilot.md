# Glossy world reflections, 5300M (glossy-review-fixes-run2-pilot)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame; chain = every Glossy Floor Chain Level pass in the frame, summed; march = Glossy Floor March Pass; guard = Glossy Mover Guard Pass; mover = Kinematic Brush Pass. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | chain ms | march ms | guard ms | mover ms | spread | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| near-4k-before-m17f | 1/1 | 0.000 | n/a | 0.064 | 0.000 | 0.000 | 0.000 | 0.000 | n/a | 31.360 | n/a | 60.956 | n/a | 61.052 | 0.096 | 1214 (1000-1229, n=69) |
| near-4k-after-m17f | 1/1 | 0.000 | n/a | 0.034 | 0.000 | 0.000 | 0.000 | 0.000 | n/a | 31.361 | n/a | 60.932 | n/a | 60.991 | 0.059 | 1216 (293-1228, n=70) |
| glossy-across-4k-after-m17f-high | 1/1 | 4.527 | n/a | 0.015 | 0.494 | 3.549 | 0.000 | 0.000 | n/a | 25.042 | n/a | 35.901 | n/a | 35.939 | 0.038 | 1187 (589-1217, n=43) |
| glossy-mover-across-4k-after-m17f-high | 1/1 | 4.170 | n/a | 0.041 | 0.531 | 3.520 | 0.045 | 5.933 | n/a | 34.408 | n/a | 51.089 | n/a | 51.298 | 0.209 | 1193 (943-1229, n=60) |
