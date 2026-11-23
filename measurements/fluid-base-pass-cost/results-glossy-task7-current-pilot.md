# Glossy world reflections, 5300M (glossy-task7-current-pilot)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-1440-after-m17f-high | 1/1 | 2.111 | n/a | 0.021 | 11.541 | n/a | 15.950 | n/a | 16.126 | 0.176 | 1108 (305-1168, n=21) |
| glossy-across-4k-after-m17f-high | 1/1 | 4.634 | n/a | 0.021 | 25.688 | n/a | 33.412 | n/a | 33.617 | 0.205 | 1202 (562-1221, n=38) |
| glossy-down-1440-after-m17f-high | 1/1 | 1.913 | n/a | 0.022 | 10.552 | n/a | 14.693 | n/a | 14.850 | 0.157 | 1039 (926-1128, n=18) |
| glossy-down-4k-after-m17f-high | 1/1 | 4.271 | n/a | 0.025 | 23.781 | n/a | 31.006 | n/a | 31.230 | 0.223 | 1202 (1002-1217, n=35) |
