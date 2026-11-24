# Glossy world reflections, 5300M (glossy-task6-flooronly)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-auto-after-m17f-high | 2/2 | 0.607 | 0.029 | 0.017 | 4.851 | 0.255 | 6.564 | 0.302 | 6.606 | 0.042 | 881 (597-1005, n=10); 921 (792-980, n=9) |
| glossy-across-1440-after-m17f-high | 2/2 | 1.432 | 0.000 | 0.015 | 10.949 | 0.003 | 13.829 | 0.022 | 13.875 | 0.046 | 1106 (989-1177, n=17); 1124 (969-1151, n=16) |
| glossy-across-4k-after-m17f-high | 2/2 | 3.214 | 0.023 | 0.015 | 24.510 | 0.016 | 30.022 | 0.066 | 30.066 | 0.043 | 1205 (1015-1216, n=33); 1198 (501-1219, n=33) |
| glossy-across-1440-before-m17f-high | 2/2 | 0.000 | 0.000 | 0.000 | 10.935 | 0.020 | 12.406 | 0.033 | 12.452 | 0.045 | 1066 (273-1205, n=17); 1047 (958-1116, n=15) |
| glossy-across-4k-before-m17f-high | 2/2 | 0.000 | 0.000 | 0.000 | 24.220 | 0.008 | 26.486 | 0.001 | 26.531 | 0.045 | 1148 (1032-1226, n=29); 1163 (374-1215, n=30) |
