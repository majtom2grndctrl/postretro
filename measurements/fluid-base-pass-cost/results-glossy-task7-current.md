# Glossy world reflections, 5300M (glossy-task7-current)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-1440-after-m17f-high | 2/2 | 2.106 | 0.004 | 0.021 | 11.551 | 0.002 | 15.986 | 0.021 | 16.116 | 0.131 | 1074 (972-1164, n=20); 1096 (923-1172, n=19) |
| glossy-across-4k-after-m17f-high | 2/2 | 4.637 | 0.005 | 0.019 | 25.666 | 0.018 | 33.356 | 0.036 | 33.584 | 0.229 | 1200 (999-1218, n=37); 1207 (330-1219, n=38) |
| glossy-down-1440-after-m17f-high | 2/2 | 1.916 | 0.002 | 0.020 | 10.594 | 0.059 | 14.664 | 0.077 | 14.819 | 0.156 | 1090 (417-1161, n=18); 1087 (933-1159, n=18) |
| glossy-down-4k-after-m17f-high | 2/2 | 4.258 | 0.018 | 0.020 | 23.793 | 0.005 | 31.028 | 0.022 | 31.212 | 0.184 | 1200 (1040-1217, n=34); 1205 (1042-1219, n=33) |
