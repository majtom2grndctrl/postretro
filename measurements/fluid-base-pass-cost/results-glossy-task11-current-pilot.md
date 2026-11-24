# Glossy world reflections, 5300M (glossy-task11-current-pilot)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame; chain = every Glossy Floor Chain Level pass in the frame, summed; march = Glossy Floor March Pass. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | chain ms | march ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-4k-after-m17f-high | 1/1 | 6.526 | n/a | 0.025 | 0.444 | 3.414 | 26.096 | n/a | 39.498 | n/a | 39.743 | 0.246 | 1200 (895-1220, n=45) |
