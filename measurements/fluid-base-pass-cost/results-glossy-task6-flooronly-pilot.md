# Glossy world reflections, 5300M (glossy-task6-flooronly-pilot)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-auto-after-m17f-high | 1/1 | 0.587 | n/a | 0.015 | 4.707 | n/a | 6.356 | n/a | 6.400 | 0.044 | 957 (837-1012, n=9) |
| glossy-across-1440-after-m17f-high | 1/1 | 1.434 | n/a | 0.015 | 10.984 | n/a | 13.886 | n/a | 13.933 | 0.048 | 1075 (978-1154, n=17) |
| glossy-across-4k-after-m17f-high | 1/1 | 3.191 | n/a | 0.015 | 24.505 | n/a | 30.009 | n/a | 30.050 | 0.041 | 1196 (1046-1219, n=33) |
| glossy-across-1440-before-m17f-high | 1/1 | 0.000 | n/a | 0.000 | 10.942 | n/a | 12.388 | n/a | 12.433 | 0.045 | 1029 (953-1108, n=15) |
| glossy-across-4k-before-m17f-high | 1/1 | 0.000 | n/a | 0.000 | 24.189 | n/a | 26.453 | n/a | 26.498 | 0.044 | 1187 (1051-1220, n=29) |
