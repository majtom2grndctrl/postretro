# Reflection blend-cost spike: Mac Metal System Trace results

- Commit: `b6a179b9ccf3a4c0aba01952ea3bacfee1cb548c` on `HEAD`; engine binary `/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/4d4fd180-7651-45c2-9e15-966dc35a45b2/scratchpad/wt-blend/target/release/postretro`
- Machine: Intel(R) Core(TM) i9-9980HK CPU @ 2.40GHz | 26.6.2
- Adapter (from the capture reports): {'name': 'AMD Radeon Pro 5300M', 'backend': 'Metal', 'device_type': 'DiscreteGpu'}
- Capture: 3840x2160, warmup 240 + 240 frames per run, POSTRETRO_SH_STREAMING=sync-proof, band 1.0 (blendw0: band 0.0)
- Idle before the batch (ioreg IOAccelerator, one value per accelerator): {"inUseVidMemoryBytes": [744894464, 744894464, 744894464, 744894464, 0, 0], "vramFreeBytes": [3031961600, 3031961600, 3031961600, 3031961600], "Device Utilization %": [0, 3, 0, 0, 7]}
- `fluid ms` (basis `complete`) is the Fluid Surface Pass GPU time per frame, averaged over steady frames (after warmup, framed by the Candidate Cull Pass; the kit named BVH Cull Pass, which this branch never records) that hold exactly one depth, forward and fluid encoder; the trace drops rows in some frames and shifts labels there. `GPU frame` is the label-independent union of every labelled interval per frame, over complete / incomplete frames: agreement means only labels were lost. `whole` is the precedent figure over the whole trace, warmup included. `clock` is the median core clock over the busy samples, with min-max.

## Per run

| arm | run | fluid ms (p10-p90) | basis | whole ms | forward ms | GPU frame ms complete/incomplete | frames complete/steady/total | clock MHz | temp C | valid |
|---|---|---|---|---|---|---|---|---|---|---|
| worst-single | 1 | 38.9241 (38.630-39.175) | complete | 38.8600 | 22.168 | 69.120/n/a | 240/240/486 | 1215 (506-1228) | 76 | yes |
| worst-blend | 1 | 66.1853 (65.621-66.627) | complete | 66.0845 | 21.634 | 94.352/n/a | 240/240/486 | 1222 (896-1230) | 80 | yes |
| worst-precomputed | 1 | 44.1837 (43.633-44.529) | complete | 44.1128 | 22.326 | 75.724/n/a | 240/240/486 | 1219 (1078-1228) | 77 | yes |
| worst-stochastic | 1 | 40.3249 (40.082-40.522) | complete | 40.2944 | 22.298 | 71.356/n/a | 240/240/486 | 1206 (1030-1230) | 76 | yes |
| typ-single | 1 | 3.6170 (3.557-3.678) | complete | 3.6082 | 19.927 | 26.285/n/a | 240/240/487 | 1129 (635-1207) | 75 | yes |
| typ-blend | 1 | 3.7417 (3.676-3.854) | complete | 3.7387 | 19.945 | 26.449/n/a | 240/240/487 | 1162 (1017-1207) | 75 | yes |
| typ-precomputed | 1 | 3.0002 (2.940-3.064) | complete | 3.0018 | 19.960 | 25.735/n/a | 240/240/487 | 1103 (925-1203) | 75 | yes |
| typ-stochastic | 1 | 3.7203 (3.666-3.765) | complete | 3.7116 | 19.912 | 26.389/n/a | 240/240/487 | 1090 (470-1214) | 74 | yes |
| e24-single | 1 | 66.3692 (66.119-66.627) | complete | 6501.4952 | 26.605 | 94.790/n/a | 240/240/5 | 1227 (1047-1231) | 79 | yes |
| e24-blend | 1 | 74.3364 (74.009-74.862) | complete | 7267.9529 | 26.613 | 102.760/n/a | 240/240/5 | 1224 (1009-1230) | 84 | yes |
| e24-precomputed | 1 | 72.8497 (72.523-73.431) | complete | 7120.5803 | 26.710 | 101.376/101.414 | 239/240/5 | 1221 (1005-1225) | 82 | yes |
| e24-stochastic | 1 | 65.0846 (64.850-65.457) | complete | 6370.1497 | 26.606 | 93.508/n/a | 240/240/5 | 1227 (1085-1231) | 81 | yes |
| typ-blendw0 | 1 | 3.6808 (3.621-3.737) | complete | 3.6779 | 19.930 | 26.376/n/a | 240/240/487 | 1150 (615-1212) | 75 | yes |
| e24-blendw0 | 1 | 67.2836 (66.956-67.695) | complete | 6575.3303 | 26.637 | 95.746/95.477 | 239/240/5 | 1226 (1059-1232) | 78 | yes |
| e24-blendw0 | 2 | 67.1079 (66.840-67.387) | complete | 6529.4897 | 26.605 | 95.530/95.221 | 238/240/5 | 1224 (1065-1231) | 80 | yes |
| typ-blendw0 | 2 | 3.6845 (3.623-3.815) | complete | 3.6691 | 19.909 | 26.363/26.106 | 239/240/487 | 1110 (1003-1208) | 76 | yes |
| e24-stochastic | 2 | 64.9703 (64.728-65.323) | complete | 6360.1417 | 26.608 | 93.393/n/a | 240/240/5 | 1227 (1105-1231) | 79 | yes |
| e24-precomputed | 2 | 72.3319 (71.933-72.796) | complete | 7089.8438 | 26.570 | 100.734/n/a | 240/240/5 | 1226 (471-1231) | 83 | yes |
| e24-blend | 2 | 74.2600 (73.921-74.790) | complete | 7276.0059 | 26.590 | 102.644/n/a | 240/240/5 | 1227 (1003-1231) | 81 | yes |
| e24-single | 2 | 66.5214 (66.275-66.837) | complete | 6511.7147 | 26.728 | 95.076/n/a | 240/240/5 | 1222 (695-1227) | 79 | yes |
| typ-stochastic | 2 | 3.7211 (3.665-3.789) | complete | 3.7114 | 19.916 | 26.398/n/a | 240/240/487 | 1169 (989-1209) | 74 | yes |
| typ-precomputed | 2 | 3.0108 (2.953-3.095) | complete | 3.0522 | 20.036 | 25.811/n/a | 240/240/487 | 1167 (979-1193) | 74 | yes |
| typ-blend | 2 | 3.7354 (3.671-3.805) | complete | 3.7327 | 19.918 | 26.421/n/a | 240/240/487 | 1165 (1089-1213) | 73 | yes |
| typ-single | 2 | 3.6333 (3.574-3.742) | complete | 3.6286 | 20.009 | 26.404/n/a | 240/240/487 | 1133 (603-1207) | 73 | yes |
| worst-stochastic | 2 | 40.3215 (40.045-40.530) | complete | 40.2575 | 22.154 | 71.186/n/a | 240/240/486 | 1214 (968-1229) | 73 | yes |
| worst-precomputed | 2 | 44.1662 (43.610-44.534) | complete | 44.1578 | 22.230 | 75.541/n/a | 240/240/486 | 1221 (1050-1228) | 76 | yes |
| worst-blend | 2 | 66.0984 (65.492-66.484) | complete | 66.0359 | 21.524 | 96.444/n/a | 240/240/486 | 1222 (1046-1229) | 78 | yes |
| worst-single | 2 | 38.9048 (38.633-39.100) | complete | 38.8685 | 22.068 | 69.183/n/a | 240/240/486 | 1214 (1012-1230) | 78 | yes |

## Per arm (valid runs)

| arm | fluid ms mean | run spread ms | vs single ms | vs single x | forward ms mean | valid/total |
|---|---|---|---|---|---|---|
| worst-single | 38.9145 | 0.0193 | 0.0000 | 1.000 | 22.118 | 2/2 |
| worst-blend | 66.1419 | 0.0869 | 27.2274 | 1.700 | 21.579 | 2/2 |
| worst-precomputed | 44.1750 | 0.0175 | 5.2605 | 1.135 | 22.278 | 2/2 |
| worst-stochastic | 40.3232 | 0.0034 | 1.4087 | 1.036 | 22.226 | 2/2 |
| typ-single | 3.6252 | 0.0163 | 0.0000 | 1.000 | 19.968 | 2/2 |
| typ-blend | 3.7386 | 0.0063 | 0.1134 | 1.031 | 19.931 | 2/2 |
| typ-precomputed | 3.0055 | 0.0106 | -0.6197 | 0.829 | 19.998 | 2/2 |
| typ-stochastic | 3.7207 | 0.0008 | 0.0955 | 1.026 | 19.914 | 2/2 |
| e24-single | 66.4453 | 0.1522 | 0.0000 | 1.000 | 26.666 | 2/2 |
| e24-blend | 74.2982 | 0.0764 | 7.8529 | 1.118 | 26.602 | 2/2 |
| e24-precomputed | 72.5908 | 0.5178 | 6.1455 | 1.092 | 26.640 | 2/2 |
| e24-stochastic | 65.0274 | 0.1143 | -1.4179 | 0.979 | 26.607 | 2/2 |
| typ-blendw0 | 3.6826 | 0.0037 | 0.0575 | 1.016 | 19.920 | 2/2 |
| e24-blendw0 | 67.1958 | 0.1757 | 0.7505 | 1.011 | 26.621 | 2/2 |
