# Glossy world reflections, 5300M (glossy-spike10-cuts)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-1440-pre-m17f-high | 1/1 | 1.639 | n/a | 0.014 | 10.967 | n/a | 14.033 | n/a | 14.063 | 0.031 | 1120 (998-1164, n=17) |
| glossy-across-1440-head-m17f-high | 1/1 | 2.173 | n/a | 0.014 | 10.970 | n/a | 14.630 | n/a | 14.666 | 0.037 | 1110 (393-1167, n=18) |
| glossy-across-1440-cutA-m17f-high | 1/1 | 2.036 | n/a | 0.014 | 10.981 | n/a | 14.503 | n/a | 14.541 | 0.038 | 1110 (324-1196, n=19) |
| glossy-across-1440-cutB-m17f-high | 1/1 | 2.008 | n/a | 0.014 | 10.969 | n/a | 14.456 | n/a | 14.493 | 0.038 | 1114 (950-1175, n=18) |
| glossy-across-1440-cutBs-m17f-high | 1/1 | 1.879 | n/a | 0.014 | 10.989 | n/a | 14.308 | n/a | 14.344 | 0.035 | 1098 (982-1148, n=17) |
| glossy-across-1440-abl_nosample-m17f-high | 1/1 | 1.931 | n/a | 0.013 | 10.984 | n/a | 14.382 | n/a | 14.421 | 0.040 | 1082 (445-1164, n=17) |
| glossy-across-1440-abl_nn-m17f-high | 0/1 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | 1122 (250-1198, n=19) |
| glossy-across-1440-abl_up-m17f-high | 1/1 | 1.995 | n/a | 0.014 | 10.969 | n/a | 14.447 | n/a | 14.488 | 0.041 | 1133 (986-1164, n=17) |
| glossy-across-1440-abl_nn_up-m17f-high | 1/1 | 1.829 | n/a | 0.014 | 10.996 | n/a | 14.300 | n/a | 14.336 | 0.035 | 1092 (288-1182, n=22) |
| glossy-across-4k-pre-m17f-high | 1/1 | 3.647 | n/a | 0.013 | 24.432 | n/a | 30.396 | n/a | 30.423 | 0.028 | 1211 (301-1220, n=34) |
| glossy-across-4k-head-m17f-high | 1/1 | 4.814 | n/a | 0.014 | 24.425 | n/a | 31.578 | n/a | 31.612 | 0.034 | 1202 (360-1231, n=35) |
| glossy-across-4k-cutA-m17f-high | 1/1 | 4.491 | n/a | 0.013 | 24.402 | n/a | 31.235 | n/a | 31.268 | 0.033 | 1181 (997-1218, n=34) |
| glossy-across-4k-cutB-m17f-high | 1/1 | 4.442 | n/a | 0.016 | 24.399 | n/a | 31.189 | n/a | 31.223 | 0.035 | 1210 (1065-1225, n=34) |
| glossy-across-4k-cutBs-m17f-high | 1/1 | 4.176 | n/a | 0.013 | 24.373 | n/a | 30.844 | n/a | 30.876 | 0.032 | 1201 (351-1218, n=35) |
| glossy-across-4k-abl_nosample-m17f-high | 1/1 | 4.289 | n/a | 0.013 | 24.507 | n/a | 31.138 | n/a | 31.170 | 0.032 | 1202 (1080-1221, n=34) |
| glossy-across-4k-abl_nn-m17f-high | 1/1 | 4.114 | n/a | 0.013 | 24.401 | n/a | 30.848 | n/a | 30.880 | 0.032 | 1203 (260-1219, n=35) |
| glossy-across-4k-abl_up-m17f-high | 1/1 | 4.416 | n/a | 0.013 | 24.409 | n/a | 31.143 | n/a | 31.175 | 0.032 | 1213 (1002-1227, n=34) |
| glossy-across-4k-abl_nn_up-m17f-high | 1/1 | 4.040 | n/a | 0.013 | 24.425 | n/a | 30.768 | n/a | 30.801 | 0.034 | 1211 (1045-1220, n=33) |
| glossy-down-4k-pre-m17f-high | 1/1 | 5.491 | n/a | 0.015 | 23.018 | n/a | 30.495 | n/a | 30.525 | 0.030 | 1172 (1026-1216, n=34) |
| glossy-down-4k-head-m17f-high | 1/1 | 7.217 | n/a | 0.015 | 22.930 | n/a | 32.173 | n/a | 32.207 | 0.034 | 1201 (1040-1218, n=35) |
| glossy-down-4k-cutA-m17f-high | 1/1 | 6.733 | n/a | 0.015 | 22.929 | n/a | 31.684 | n/a | 31.690 | 0.006 | 1181 (1067-1217, n=35) |
| glossy-down-4k-cutB-m17f-high | 1/1 | 6.633 | n/a | 0.015 | 22.933 | n/a | 31.587 | n/a | 31.620 | 0.033 | 1192 (1004-1231, n=34) |
| glossy-down-4k-cutBs-m17f-high | 1/1 | 6.228 | n/a | 0.014 | 23.026 | n/a | 31.268 | n/a | 31.299 | 0.032 | 1181 (335-1217, n=35) |
| glossy-down-4k-abl_nosample-m17f-high | 1/1 | 6.383 | n/a | 0.015 | 22.939 | n/a | 31.331 | n/a | 31.366 | 0.035 | 1174 (1015-1215, n=34) |
| glossy-down-4k-abl_nn-m17f-high | 1/1 | 6.165 | n/a | 0.015 | 22.935 | n/a | 31.085 | n/a | 31.118 | 0.033 | 1199 (1029-1216, n=34) |
| glossy-down-4k-abl_up-m17f-high | 1/1 | 6.595 | n/a | 0.015 | 22.940 | n/a | 31.538 | n/a | 31.577 | 0.039 | 1176 (961-1217, n=35) |
| glossy-down-4k-abl_nn_up-m17f-high | 1/1 | 6.070 | n/a | 0.014 | 22.941 | n/a | 31.018 | n/a | 31.050 | 0.032 | 1199 (1037-1220, n=34) |

Invalid runs:
- glossy-spike10-cuts-glossy-across-1440-abl_nn-m17f-high-r1: GPU frame time differs between complete (14.3077 ms) and incomplete (15.4133 ms) frames: dropped rows lost time, not only labels
