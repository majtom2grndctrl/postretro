# Glossy world reflections, 5300M (glossy-spike10-current-pilot)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-1440-after-m17f-high | 1/1 | 2.909 | n/a | 0.019 | 11.508 | n/a | 16.736 | n/a | 16.859 | 0.122 | 1089 (403-1161, n=21) |
