# Glossy world reflections, 5300M (glossy-spike10-flooronly)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-auto-after-m17f-high | 2/2 | 0.784 | 0.001 | 0.014 | 4.578 | 0.020 | 6.385 | 0.028 | 6.415 | 0.030 | 935 (881-1004, n=9); 997 (855-1092, n=9) |
| glossy-across-1440-after-m17f-high | 2/2 | 1.946 | 0.000 | 0.013 | 10.975 | 0.002 | 14.368 | 0.006 | 14.389 | 0.020 | 1130 (970-1175, n=18); 1138 (382-1161, n=18) |
| glossy-across-4k-after-m17f-high | 2/2 | 4.317 | 0.009 | 0.014 | 24.478 | 0.032 | 31.121 | 0.050 | 31.150 | 0.029 | 1207 (289-1221, n=36); 1202 (321-1222, n=35) |
| glossy-down-auto-after-m17f-high | 2/2 | 1.236 | 0.046 | 0.015 | 4.262 | 0.165 | 6.286 | 0.220 | 6.305 | 0.019 | 967 (914-1032, n=9); 862 (337-988, n=10) |
| glossy-down-1440-after-m17f-high | 2/2 | 2.907 | 0.000 | 0.016 | 10.250 | 0.005 | 14.340 | 0.011 | 14.378 | 0.038 | 1087 (314-1158, n=18); 1132 (979-1154, n=17) |
| glossy-down-4k-after-m17f-high | 2/2 | 6.453 | 0.007 | 0.015 | 22.992 | 0.012 | 31.414 | 0.003 | 31.445 | 0.031 | 1191 (1037-1227, n=35); 1193 (259-1220, n=36) |
| glossy-across-1440-before-m17f-high | 2/2 | 0.000 | 0.000 | 0.000 | 10.952 | 0.018 | 12.406 | 0.027 | 12.439 | 0.034 | 1076 (945-1144, n=15); 1098 (951-1144, n=15) |
| glossy-across-4k-before-m17f-high | 2/2 | 0.000 | 0.000 | 0.000 | 24.136 | 0.002 | 26.425 | 0.024 | 26.437 | 0.012 | 1186 (1051-1227, n=29); 1169 (1025-1222, n=29) |
| glossy-across-1440-head-m17f-high | 2/2 | 2.174 | 0.002 | 0.014 | 10.973 | 0.012 | 14.615 | 0.001 | 14.636 | 0.020 | 1122 (330-1183, n=18); 1121 (1040-1165, n=18) |
| glossy-across-4k-head-m17f-high | 2/2 | 4.820 | 0.005 | 0.013 | 24.480 | 0.001 | 31.631 | 0.013 | 31.649 | 0.018 | 1206 (1081-1220, n=35); 1198 (465-1220, n=35) |
| glossy-down-4k-head-m17f-high | 2/2 | 7.231 | 0.010 | 0.016 | 22.947 | 0.082 | 32.162 | 0.043 | 32.194 | 0.032 | 1189 (1003-1218, n=35); 1187 (1041-1222, n=35) |
