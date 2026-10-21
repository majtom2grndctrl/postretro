# Fluid base-pass ablation (landing2-pilot)

Fluid = Fluid Surface Pass per complete frame; frame = union of every labelled interval per complete frame; clock = median busy core clock (min-max) per run.

| arm | runs valid | fluid ms | spread | forward ms | GPU frame ms | clocks |
|---|---|---|---|---|---|---|
| near-4k-before | 1/1 | 53.095 | n/a | 23.995 | 78.900 | 1213 (299-1223) |
| near-4k-after | 1/1 | 2.161 | n/a | 23.906 | 27.873 | 1156 (1053-1205) |
| test-4k-before | 1/1 | 21.374 | n/a | 20.201 | 50.343 | 1192 (1053-1218) |
| test-4k-after | 1/1 | 2.133 | n/a | 19.322 | 25.893 | 1180 (1021-1205) |
| mid-4k-before | 1/1 | 18.502 | n/a | 20.270 | 40.835 | 1204 (1037-1222) |
| mid-4k-after | 1/1 | 1.248 | n/a | 20.232 | 23.495 | 1104 (880-1145) |
| near-4k-before-m17f | 1/1 | 63.953 | n/a | 26.688 | 92.486 | 1226 (495-1230) |
| near-4k-after-m17f | 1/1 | 12.309 | n/a | 26.939 | 41.173 | 1154 (747-1222) |
