# Glossy world reflections, 5300M (glossy-task10-current-pilot)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-1440-after-m17f-high | 1/1 | 3.123 | n/a | 0.019 | 11.498 | n/a | 16.904 | n/a | 17.071 | 0.167 | 1076 (961-1185, n=21) |
| glossy-across-4k-after-m17f-high | 1/1 | 6.846 | n/a | 0.019 | 25.578 | n/a | 35.586 | n/a | 35.847 | 0.261 | 1186 (963-1221, n=40) |
| glossy-down-1440-after-m17f-high | 1/1 | 2.531 | n/a | 0.020 | 10.562 | n/a | 15.285 | n/a | 15.438 | 0.153 | 1087 (968-1151, n=18) |
| glossy-down-4k-after-m17f-high | 1/1 | 5.628 | n/a | 0.019 | 23.749 | n/a | 32.404 | n/a | 32.621 | 0.217 | 1208 (1061-1216, n=37) |
