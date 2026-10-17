# Cell-rate spike: Mac Metal System Trace results

- Commit: `fd176433cbb69eb8f01669781a29b51d97d61c8d` on `claude/cell-rate-spike`; engine binary `/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/1f2c925e-d6a0-4b30-82ef-11de1519cd43/scratchpad/bin/1660-cab5197a5/postretro`
- Machine: Intel(R) Core(TM) i9-9980HK CPU @ 2.40GHz | 26.6.2
- Adapter (from the capture reports): {'name': 'AMD Radeon Pro 5300M', 'backend': 'Metal', 'device_type': 'DiscreteGpu'}
- Capture: warmup 240 + 240 frames per run, POSTRETRO_SH_STREAMING=sync-proof
- Poses: {"near": {"map": "content/dev/maps/e24-environment-test.prl", "position": [-4.8768, 1.6, -44.2976], "yaw": 0.0, "pitch": -89.0}, "test": {"map": "content/dev/maps/reflection-probes-test.prl", "position": [-6.5024, 1.6, -6.5024], "yaw": 0.0, "pitch": -89.0}, "far": {"map": "content/dev/maps/e24-environment-test.prl", "position": [-4.877, 4.0, -40.64], "yaw": 0.0, "pitch": 5.0}, "low": {"map": "content/dev/maps/e24-environment-test.prl", "position": [-4.8768, 1.6, -44.2976], "yaw": 0.0, "pitch": -15.0}, "mid": {"map": "content/dev/maps/e24-environment-test.prl", "position": [-4.8768, 1.6, -44.2976], "yaw": 0.0, "pitch": -30.0}, "tilt": {"map": "content/dev/maps/e24-environment-test.prl", "position": [-4.8768, 1.6, -44.2976], "yaw": 0.0, "pitch": -45.0}}
- Idle before the batch (ioreg IOAccelerator, one value per accelerator): {"inUseVidMemoryBytes": [901963776, 901963776, 901963776, 901963776, 0, 0], "vramFreeBytes": [2717679616, 2717679616, 2717679616, 2717679616], "Device Utilization %": [0, 0, 0, 0, 1]}
- `cost ms` is Fluid Surface Pass + Fluid Reflection Cell Pass GPU time per frame, averaged over complete steady frames (the last `samples` frames framed by the Candidate Cull Pass that hold exactly one depth, forward and fluid encoder, and exactly one cell-pass encoder in cell arms, none in frag arms). `GPU frame` is the label-independent union of every labelled interval per frame over complete / incomplete frames. `clock` is the median core clock over busy samples, with min-max. Modes: `frag` per-fragment pixel style; `cells` cell buffer; `cull` cell buffer with per-cell frustum cull; `-r1` the live-lighting ratio forced to 1 in both paths.

## Per run

| arm | run | cost ms (p10-p90) | fluid ms | cell ms | forward ms | GPU frame ms complete/incomplete | frames complete/steady/total | clock MHz | temp C | valid |
|---|---|---|---|---|---|---|---|---|---|---|
| near-4k-cell2-r1 | 1 | 58.2496 (58.131-58.326) | 58.0844 | 0.1652 | 26.624 | 86.548/86.392 | 239/240/5 | 1222 (1031-1230) | 81 | yes |
| near-4k-cell2-r1 | 2 | 57.8259 (57.640-57.966) | 57.6636 | 0.1624 | 26.640 | 86.111/n/a | 240/240/5 | 1225 (1004-1232) | 80 | yes |
| near-4k-cell2 | 1 | 57.6797 (57.529-57.791) | 57.5076 | 0.1721 | 26.646 | 85.980/n/a | 240/240/5 | 1174 (1049-1229) | 80 | yes |
| near-4k-cell2 | 2 | 58.0772 (57.873-58.503) | 57.9028 | 0.1744 | 26.746 | 86.474/n/a | 240/240/5 | 1222 (1019-1226) | 80 | yes |
| near-4k-frag-r1 | 1 | 66.1012 (65.782-66.525) | 66.1012 | 0.0000 | 26.694 | 94.634/n/a | 240/240/5 | 1222 (1051-1227) | 80 | yes |
| near-4k-frag-r1 | 2 | 65.4205 (65.150-65.700) | 65.4205 | 0.0000 | 26.607 | 93.882/n/a | 240/240/5 | 1227 (1038-1231) | 79 | yes |
| near-4k-frag | 1 | 66.4754 (66.215-66.795) | 66.4754 | 0.0000 | 26.613 | 94.938/n/a | 240/240/5 | 1227 (920-1231) | 79 | yes |
| near-4k-frag | 2 | 66.8572 (66.550-67.245) | 66.8572 | 0.0000 | 26.718 | 95.392/n/a | 240/240/5 | 1222 (1010-1226) | 80 | yes |
| near-4k-frag2-r1 | 1 | 66.5011 (66.299-66.683) | 66.5011 | 0.0000 | 26.621 | 94.927/n/a | 240/240/5 | 1226 (1034-1229) | 82 | yes |
| near-4k-frag2-r1 | 2 | 66.3522 (66.118-66.541) | 66.3522 | 0.0000 | 26.581 | 94.736/n/a | 240/240/5 | 1226 (1058-1232) | 82 | yes |
| near-4k-frag2 | 1 | 66.9160 (66.678-67.097) | 66.9160 | 0.0000 | 26.584 | 95.322/n/a | 240/240/5 | 1227 (1021-1230) | 80 | yes |
| near-4k-frag2 | 2 | 67.0375 (66.809-67.204) | 67.0375 | 0.0000 | 26.588 | 95.434/95.384 | 239/240/5 | 1226 (1038-1230) | 80 | yes |
| near-4k-free | 1 | 57.5366 (57.436-57.619) | 57.5366 | 0.0000 | 26.568 | 85.911/n/a | 240/240/5 | 1222 (278-1230) | 80 | yes |
| near-4k-free | 2 | 57.1990 (57.052-57.322) | 57.1990 | 0.0000 | 26.577 | 85.586/n/a | 240/240/5 | 1222 (875-1229) | 79 | yes |
| test-4k-cell2-r1 | 1 | 25.2622 (23.481-27.226) | 24.8617 | 0.4004 | 22.095 | 55.872/n/a | 240/240/490 | 1212 (1006-1230) | 76 | yes |
| test-4k-cell2-r1 | 2 | 25.2584 (23.519-27.168) | 24.7812 | 0.4772 | 22.079 | 56.094/n/a | 240/240/490 | 1217 (534-1229) | 75 | yes |
| test-4k-cell2 | 1 | 25.5072 (23.548-27.317) | 24.9373 | 0.5699 | 22.263 | 56.083/n/a | 240/240/490 | 1216 (990-1228) | 74 | yes |
| test-4k-cell2 | 2 | 25.5221 (23.644-27.339) | 24.8511 | 0.6710 | 22.379 | 56.413/n/a | 240/240/490 | 1213 (1004-1224) | 75 | yes |
| test-4k-frag-r1 | 1 | 31.5361 (31.331-31.742) | 31.5361 | 0.0000 | 21.395 | 62.848/n/a | 240/240/490 | 1206 (1058-1230) | 75 | yes |
| test-4k-frag-r1 | 2 | 31.5445 (31.334-31.769) | 31.5445 | 0.0000 | 21.413 | 62.922/n/a | 240/240/490 | 1192 (1088-1231) | 75 | yes |
| test-4k-frag | 1 | 36.7536 (36.538-36.968) | 36.7536 | 0.0000 | 21.870 | 66.702/n/a | 240/240/490 | 1215 (1021-1230) | 76 | yes |
| test-4k-frag | 2 | 36.7615 (36.565-36.959) | 36.7615 | 0.0000 | 21.900 | 66.859/n/a | 240/240/490 | 1219 (1094-1230) | 77 | yes |
| test-4k-frag2-r1 | 1 | 32.0925 (31.834-32.314) | 32.0925 | 0.0000 | 21.577 | 61.085/n/a | 240/240/490 | 1219 (1001-1232) | 78 | yes |
| test-4k-frag2-r1 | 2 | 32.0170 (31.826-32.224) | 32.0170 | 0.0000 | 21.453 | 62.849/n/a | 240/240/490 | 1184 (1086-1229) | 76 | yes |
| test-4k-frag2 | 1 | 35.7629 (35.515-35.989) | 35.7629 | 0.0000 | 21.747 | 64.877/n/a | 240/240/490 | 1221 (1013-1229) | 75 | yes |
| test-4k-frag2 | 2 | 35.7389 (35.518-35.962) | 35.7389 | 0.0000 | 21.718 | 65.118/n/a | 240/240/490 | 1216 (1026-1230) | 75 | yes |
| test-4k-free | 1 | 23.1775 (22.960-23.399) | 23.1775 | 0.0000 | 22.540 | 54.903/n/a | 240/240/490 | 1213 (986-1224) | 74 | yes |
| test-4k-free | 2 | 23.0763 (22.853-23.266) | 23.0763 | 0.0000 | 22.450 | 54.888/n/a | 240/240/490 | 1218 (1001-1230) | 74 | yes |

## Per arm (valid runs)

| arm | cost ms mean | run spread ms | fluid ms | cell ms | vs frag ms | vs frag x | forward ms | valid/total |
|---|---|---|---|---|---|---|---|---|
| near-4k-cell2-r1 | 58.0378 | 0.4237 | 57.8740 | 0.1638 | -7.7231 | 0.883 | 26.632 | 2/2 |
| near-4k-cell2 | 57.8785 | 0.3975 | 57.7052 | 0.1733 | -8.7879 | 0.868 | 26.696 | 2/2 |
| near-4k-frag-r1 | 65.7609 | 0.6807 | 65.7609 | 0.0000 | 0.0000 | 1.000 | 26.651 | 2/2 |
| near-4k-frag | 66.6663 | 0.3818 | 66.6663 | 0.0000 | 0.0000 | 1.000 | 26.666 | 2/2 |
| near-4k-frag2-r1 | 66.4266 | 0.1489 | 66.4266 | 0.0000 | 0.6658 | 1.010 | 26.601 | 2/2 |
| near-4k-frag2 | 66.9767 | 0.1215 | 66.9767 | 0.0000 | 0.3104 | 1.005 | 26.586 | 2/2 |
| near-4k-free | 57.3678 | 0.3376 | 57.3678 | 0.0000 | -9.2985 | 0.861 | 26.573 | 2/2 |
| test-4k-cell2-r1 | 25.2603 | 0.0038 | 24.8214 | 0.4388 | -6.2800 | 0.801 | 22.087 | 2/2 |
| test-4k-cell2 | 25.5146 | 0.0149 | 24.8942 | 0.6204 | -11.2429 | 0.694 | 22.321 | 2/2 |
| test-4k-frag-r1 | 31.5403 | 0.0084 | 31.5403 | 0.0000 | 0.0000 | 1.000 | 21.404 | 2/2 |
| test-4k-frag | 36.7575 | 0.0079 | 36.7575 | 0.0000 | 0.0000 | 1.000 | 21.885 | 2/2 |
| test-4k-frag2-r1 | 32.0547 | 0.0755 | 32.0547 | 0.0000 | 0.5144 | 1.016 | 21.515 | 2/2 |
| test-4k-frag2 | 35.7509 | 0.0240 | 35.7509 | 0.0000 | -1.0066 | 0.973 | 21.732 | 2/2 |
| test-4k-free | 23.1269 | 0.1012 | 23.1269 | 0.0000 | -13.6306 | 0.629 | 22.495 | 2/2 |
