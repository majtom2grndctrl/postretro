# Glossy world reflections, 5300M (glossy-task12-m7-pilot)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame; chain = every Glossy Floor Chain Level pass in the frame, summed; march = Glossy Floor March Pass; mover = Kinematic Brush Pass. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | chain ms | march ms | mover ms | spread | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-mover-across-4k-after-m17f-high-specpixel | 1/1 | 4.182 | n/a | 0.039 | 0.529 | 3.558 | 5.927 | n/a | 34.439 | n/a | 51.092 | n/a | 51.317 | 0.225 | 1198 (255-1222, n=58) |
| glossy-mover-across-1440-after-m17f-low-specsmooth | 1/1 | 1.822 | n/a | 0.040 | 0.000 | 0.000 | 2.439 | n/a | 14.805 | n/a | 20.832 | n/a | 21.050 | 0.218 | 1125 (314-1225, n=26) |
| glossy-mover-across-4k-before-m17f-high-specsmooth | 1/1 | 0.000 | n/a | 0.000 | 0.000 | 0.000 | 5.616 | n/a | 32.572 | n/a | 40.660 | n/a | 40.846 | 0.186 | 1201 (259-1220, n=46) |
