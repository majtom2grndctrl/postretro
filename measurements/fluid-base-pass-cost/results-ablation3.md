# Fluid base-pass ablation (ablation3)

Fluid = Fluid Surface Pass per complete frame; frame = union of every labelled interval per complete frame; clock = median busy core clock (min-max) per run.

| arm | runs valid | fluid ms | spread | forward ms | GPU frame ms | clocks |
|---|---|---|---|---|---|---|
| near-4k-base | 2/2 | 55.024 | 0.031 | 23.871 | 80.741 | 1227 (1034-1229); 1225 (651-1229) |
| near-4k-vertexsh | 2/2 | 2.107 | 0.009 | 23.928 | 27.990 | 1155 (986-1211); 1153 (624-1199) |
| test-4k-base | 2/2 | 22.272 | 0.005 | 20.107 | 51.249 | 1195 (1008-1228); 1211 (530-1226) |
| test-4k-vertexsh | 2/2 | 2.123 | 0.002 | 19.236 | 25.742 | 1156 (1016-1206); 1177 (857-1214) |
| near-1080-base | 2/2 | 13.915 | 0.038 | 6.132 | 20.948 | 1139 (988-1224); 1141 (275-1177) |
| near-1080-vertexsh | 2/2 | 0.577 | 0.015 | 6.406 | 7.853 | 999 (490-1069); 944 (305-1037) |
