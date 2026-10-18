# Fluid base-pass ablation (ablation2)

Fluid = Fluid Surface Pass per complete frame; frame = union of every labelled interval per complete frame; clock = median busy core clock (min-max) per run.

| arm | runs valid | fluid ms | spread | forward ms | GPU frame ms | clocks |
|---|---|---|---|---|---|---|
| near-4k-base | 2/2 | 55.327 | 0.562 | 23.771 | 80.964 | 1222 (494-1229); 1218 (925-1229) |
| near-4k-shshared | 2/2 | 53.475 | 0.020 | 25.732 | 82.709 | 1221 (602-1230); 1227 (467-1234) |
| near-4k-noocc | 2/2 | 73.330 | 0.023 | 25.661 | 102.424 | 1228 (271-1234); 1227 (1048-1231) |
| test-4k-base | 2/2 | 22.314 | 0.093 | 20.146 | 51.437 | 1192 (1010-1224); 1176 (949-1225) |
| test-4k-shshared | 2/2 | 26.828 | 0.061 | 19.747 | 54.841 | 1218 (1069-1230); 1192 (948-1230) |
| test-4k-noocc | 2/2 | 33.113 | 0.010 | 19.428 | 62.169 | 1205 (949-1233); 1206 (993-1231) |
