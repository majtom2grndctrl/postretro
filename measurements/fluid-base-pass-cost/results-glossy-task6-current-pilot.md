# Glossy world reflections, 5300M (glossy-task6-current-pilot)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-1440-after-m17f-high | 1/1 | 1.883 | n/a | 0.020 | 11.618 | n/a | 16.162 | n/a | 16.335 | 0.173 | 1067 (935-1174, n=20) |
| glossy-across-4k-after-m17f-high | 1/1 | 4.138 | n/a | 0.019 | 25.836 | n/a | 33.403 | n/a | 33.679 | 0.276 | 1201 (1054-1220, n=38) |
| glossy-down-1440-after-m17f-high | 1/1 | 1.889 | n/a | 0.021 | 10.596 | n/a | 14.874 | n/a | 15.053 | 0.179 | 1080 (948-1203, n=19) |
| glossy-down-4k-after-m17f-high | 1/1 | 4.210 | n/a | 0.022 | 23.787 | n/a | 31.252 | n/a | 31.505 | 0.253 | 1190 (991-1216, n=35) |
