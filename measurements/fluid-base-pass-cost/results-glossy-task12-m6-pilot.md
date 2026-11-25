# Glossy world reflections, 5300M (glossy-task12-m6-pilot)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame; chain = every Glossy Floor Chain Level pass in the frame, summed; march = Glossy Floor March Pass; mover = Kinematic Brush Pass. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | chain ms | march ms | mover ms | spread | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| near-4k-before-m17f | 1/1 | 0.000 | n/a | 0.059 | 0.000 | 0.000 | 0.000 | n/a | 31.320 | n/a | 60.833 | n/a | 60.922 | 0.088 | 1215 (378-1226, n=68) |
| near-4k-after-m17f | 1/1 | 0.000 | n/a | 0.033 | 0.000 | 0.000 | 0.000 | n/a | 31.303 | n/a | 61.338 | n/a | 61.397 | 0.059 | 1219 (1061-1226, n=67) |
| glossy-across-4k-before-m17f-high | 1/1 | 0.000 | n/a | 0.000 | 0.000 | 0.000 | 0.000 | n/a | 25.279 | n/a | 28.373 | n/a | 28.591 | 0.218 | 1162 (343-1213, n=33) |
| glossy-across-1440-after-m17f-off | 1/1 | 0.000 | n/a | 0.000 | 0.000 | 0.000 | 0.000 | n/a | 11.547 | n/a | 13.796 | n/a | 13.915 | 0.118 | 1096 (976-1147, n=16) |
