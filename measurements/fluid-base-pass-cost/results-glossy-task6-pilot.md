# Glossy world reflections, 5300M (glossy-task6-pilot)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-auto-after-m17f-high | 1/1 | 0.827 | n/a | 0.018 | 4.934 | n/a | 6.844 | n/a | 6.882 | 0.039 | 918 (796-1016, n=9) |
| glossy-across-1440-after-m17f-high | 1/1 | 1.982 | n/a | 0.014 | 11.441 | n/a | 14.862 | n/a | 14.903 | 0.041 | 1120 (531-1182, n=18) |
| glossy-across-4k-after-m17f-high | 1/1 | 4.348 | n/a | 0.015 | 25.522 | n/a | 32.081 | n/a | 32.122 | 0.041 | 1200 (302-1222, n=37) |
| glossy-down-auto-after-m17f-high | 0/1 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | 846 (684-1001, n=10) |

Invalid runs:
- glossy-task6-pilot-glossy-down-auto-after-m17f-high-r1: GPU frame time differs between complete (6.4928 ms) and incomplete (6.8575 ms) frames: dropped rows lost time, not only labels
