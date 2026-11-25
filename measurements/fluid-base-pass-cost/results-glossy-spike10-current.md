# Glossy world reflections, 5300M (glossy-spike10-current)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-1440-after-m17f-high | 2/2 | 2.909 | 0.000 | 0.020 | 11.536 | 0.040 | 16.740 | 0.051 | 16.886 | 0.146 | 1084 (932-1159, n=20); 1095 (457-1168, n=21) |
| glossy-across-4k-after-m17f-high | 2/2 | 6.362 | 0.006 | 0.019 | 25.624 | 0.066 | 35.089 | 0.013 | 35.333 | 0.244 | 1209 (306-1219, n=41); 1206 (438-1219, n=39) |
| glossy-down-1440-after-m17f-high | 2/2 | 2.253 | 0.002 | 0.020 | 10.567 | 0.018 | 14.988 | 0.019 | 15.141 | 0.153 | 1107 (287-1160, n=20); 1105 (958-1160, n=18) |
| glossy-down-4k-after-m17f-high | 2/2 | 5.036 | 0.019 | 0.022 | 23.861 | 0.096 | 31.905 | 0.147 | 32.125 | 0.220 | 1202 (500-1224, n=37); 1203 (973-1225, n=35) |
