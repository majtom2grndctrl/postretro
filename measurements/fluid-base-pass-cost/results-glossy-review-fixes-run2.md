# Glossy world reflections, 5300M (glossy-review-fixes-run2)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame; chain = every Glossy Floor Chain Level pass in the frame, summed; march = Glossy Floor March Pass; guard = Glossy Mover Guard Pass; mover = Kinematic Brush Pass. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | chain ms | march ms | guard ms | mover ms | spread | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| near-1440-before-m17f | 2/2 | 0.000 | 0.000 | 0.048 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 14.085 | 0.017 | 27.771 | 0.047 | 27.860 | 0.089 | 1162 (973-1226, n=33); 1132 (289-1224, n=34) |
| near-1440-after-m17f | 2/2 | 0.000 | 0.000 | 0.037 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 14.161 | 0.044 | 27.834 | 0.028 | 27.905 | 0.071 | 1130 (873-1204, n=33); 1121 (969-1210, n=33) |
| near-4k-before-m17f | 2/2 | 0.000 | 0.000 | 0.058 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 31.247 | 0.042 | 60.786 | 0.108 | 60.873 | 0.087 | 1218 (1085-1229, n=68); 1218 (998-1229, n=68) |
| near-4k-after-m17f | 2/2 | 0.000 | 0.000 | 0.036 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 31.296 | 0.327 | 60.923 | 0.454 | 60.984 | 0.061 | 1219 (1026-1226, n=68); 1212 (999-1222, n=68) |
| glossy-across-1440-after-m17f-high | 2/2 | 2.054 | 0.002 | 0.015 | 0.368 | 1.589 | 0.000 | 0.000 | 0.000 | 11.143 | 0.002 | 16.615 | 0.003 | 16.652 | 0.037 | 1083 (247-1138, n=23); 1114 (283-1161, n=24) |
| glossy-down-1440-after-m17f-high | 2/2 | 3.061 | 0.003 | 0.019 | 1.167 | 1.901 | 0.000 | 0.000 | 0.000 | 9.468 | 0.818 | 16.802 | 0.029 | 16.844 | 0.042 | 1075 (920-1201, n=21); 1063 (447-1166, n=22) |
| glossy-across-4k-after-m17f-high | 2/2 | 4.527 | 0.009 | 0.014 | 0.505 | 3.548 | 0.000 | 0.000 | 0.000 | 25.026 | 0.019 | 35.908 | 0.025 | 35.943 | 0.034 | 1184 (280-1220, n=43); 1178 (272-1227, n=43) |
| glossy-down-4k-after-m17f-high | 2/2 | 6.817 | 0.010 | 0.018 | 0.468 | 4.153 | 0.000 | 0.000 | 0.000 | 23.389 | 0.009 | 36.825 | 0.031 | 36.868 | 0.043 | 1125 (277-1228, n=44); 1183 (1036-1217, n=43) |
| glossy-mover-across-1440-after-m17f-high | 2/2 | 1.900 | 0.006 | 0.047 | 0.310 | 1.609 | 0.090 | 2.657 | 0.054 | 15.647 | 0.190 | 23.929 | 0.192 | 24.147 | 0.218 | 1140 (455-1212, n=31); 1082 (926-1187, n=30) |
| glossy-mover-across-4k-after-m17f-high | 2/2 | 4.182 | 0.002 | 0.042 | 0.534 | 3.511 | 0.053 | 5.951 | 0.049 | 34.381 | 0.070 | 51.102 | 0.031 | 51.312 | 0.211 | 1194 (305-1231, n=61); 1203 (323-1231, n=59) |
