# Glossy world reflections, 5300M (glossy-spike10-s1)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-1440-head-m17f-high | 1/1 | 2.178 | n/a | 0.014 | 10.981 | n/a | 14.681 | n/a | 14.717 | 0.036 | 1118 (1010-1177, n=18) |
| glossy-across-1440-cutA-m17f-high | 1/1 | 2.038 | n/a | 0.014 | 10.972 | n/a | 14.482 | n/a | 14.518 | 0.037 | 1126 (296-1163, n=18) |
| glossy-across-1440-cutB-m17f-high | 1/1 | 2.004 | n/a | 0.014 | 10.982 | n/a | 14.441 | n/a | 14.477 | 0.037 | 1133 (299-1204, n=18) |
| glossy-across-4k-head-m17f-high | 0/1 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | 1203 (1055-1225, n=34) |

Invalid runs:
- glossy-spike10-s1-glossy-across-4k-head-m17f-high-r1: xctrace still running 180 s after the engine finished; interrupted; xctrace ignored SIGINT for 180 s; terminated; xctrace export failed: Export failed: Document Missing Template Error
Export failed: Unexpected internal error
