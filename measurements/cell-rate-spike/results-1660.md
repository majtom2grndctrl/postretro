# Fluid reflection cell-rate spike: 1660 timestamp-query results

- Commit: `65ca0fbb3946014409dfbc4c8ee899956e828584` on `claude/cell-rate-spike-1660`; engine binary `C:\Users\danhi\Projects\Personal\postretro\.claude\worktrees\reflection-probes-ssr-research-42d47a\target\release\postretro.exe`
- Machine: 1660
- Adapter (from the capture reports): {'name': 'NVIDIA GeForce GTX 1660 SUPER', 'backend': 'Vulkan', 'device_type': 'DiscreteGpu'}
- Capture: warmup 240 + 360 frames per run, POSTRETRO_SH_STREAMING=sync-proof, pixel style, reflectivity 1 (capture's fixed look)
- Idle before the batch (ioreg IOAccelerator, one value per accelerator): null
- `cost ms` (basis `complete`) is Fluid Surface Pass + Fluid Reflection Cell Pass GPU time per frame, averaged over steady frames (the last `samples` frames framed by the Candidate Cull Pass) that hold exactly one depth, forward and fluid encoder, and one cell encoder in a cell arm; the trace drops rows in some frames and shifts labels there. `GPU frame` is the label-independent union of every labelled interval per frame over complete / incomplete frames: agreement means only labels were lost. `clock` is the median core clock over the busy samples, with min-max. Modes: `frag` per-fragment pixel style; `cell` the cell buffer; `cellcull` the cell buffer with its per-cell frustum cull. Ratios: `live` the live-lighting ratio as shipped; `r1` forced to 1 in both paths.

## Per arm (valid runs)

| arm | cost ms mean | run spread ms | fluid pass ms | cell pass ms | vs frag ms | vs frag x | forward ms | valid/total |
|---|---|---|---|---|---|---|---|---|
| near-1080p-frag-live | 5.7342 | 0.0364 | 5.7342 | 0.0000 | 0.0000 | 1.000 | 2.165 | 2/2 |
| near-1080p-cell-live | 5.3255 | 0.0215 | 5.2962 | 0.0292 | -0.4088 | 0.929 | 2.156 | 2/2 |
| near-1080p-cellcull-live | 5.3174 | 0.0120 | 5.2970 | 0.0204 | -0.4168 | 0.927 | 2.158 | 2/2 |
| near-1080p-cell2-live | 3.5041 | 0.0064 | 3.4749 | 0.0292 | -2.2301 | 0.611 | 2.152 | 2/2 |
| near-1080p-frag2-live | 3.9510 | 0.0543 | 3.9510 | 0.0000 | -1.7832 | 0.689 | 2.200 | 2/2 |
| near-1080p-free-live | 3.1100 | 0.0110 | 3.1100 | 0.0000 | -2.6242 | 0.542 | 2.146 | 2/2 |
| near-1080p-frag-r1 | 5.2662 | 0.0265 | 5.2662 | 0.0000 | 0.0000 | 1.000 | 2.154 | 2/2 |
| near-1080p-cell-r1 | 5.3243 | 0.0112 | 5.2966 | 0.0276 | 0.0580 | 1.011 | 2.156 | 2/2 |
| near-1080p-cellcull-r1 | 5.3130 | 0.0111 | 5.2936 | 0.0195 | 0.0468 | 1.009 | 2.160 | 2/2 |
| near-1080p-cell2-r1 | 3.5047 | 0.0088 | 3.4771 | 0.0276 | -1.7615 | 0.666 | 2.152 | 2/2 |
| near-1080p-frag2-r1 | 3.9324 | 0.1727 | 3.9324 | 0.0000 | -1.3338 | 0.747 | 2.199 | 2/2 |
| near-4k-frag-live | 22.2379 | 0.2181 | 22.2379 | 0.0000 | 0.0000 | 1.000 | 8.383 | 2/2 |
| near-4k-cell-live | 20.6191 | 0.1348 | 20.5883 | 0.0308 | -1.6188 | 0.927 | 8.375 | 2/2 |
| near-4k-cellcull-live | 20.5966 | 0.2511 | 20.5748 | 0.0218 | -1.6413 | 0.926 | 8.389 | 2/2 |
| near-4k-cell2-live | 12.8670 | 0.0761 | 12.8363 | 0.0306 | -9.3709 | 0.579 | 8.309 | 2/2 |
| near-4k-frag2-live | 14.9688 | 0.0980 | 14.9688 | 0.0000 | -7.2692 | 0.673 | 8.290 | 2/2 |
| near-4k-free-live | 12.2424 | 0.0060 | 12.2424 | 0.0000 | -9.9955 | 0.551 | 8.117 | 2/2 |
| near-4k-frag-r1 | 20.6497 | 0.0216 | 20.6497 | 0.0000 | 0.0000 | 1.000 | 8.318 | 2/2 |
| near-4k-cell-r1 | 21.2225 | 0.0447 | 21.1934 | 0.0290 | 0.5727 | 1.028 | 8.488 | 2/2 |
| near-4k-cellcull-r1 | 20.4221 | 0.0959 | 20.4014 | 0.0207 | -0.2276 | 0.989 | 8.303 | 2/2 |
| near-4k-cell2-r1 | 12.6606 | 0.0796 | 12.6316 | 0.0290 | -7.9891 | 0.613 | 8.196 | 2/2 |
| near-4k-frag2-r1 | 14.6554 | 0.0180 | 14.6554 | 0.0000 | -5.9943 | 0.710 | 8.301 | 2/2 |
| probes-4k-frag-live | 10.8505 | 0.0390 | 10.8505 | 0.0000 | 0.0000 | 1.000 | 6.792 | 2/2 |
| probes-4k-cell-live | 6.2285 | 0.0019 | 6.2015 | 0.0270 | -4.6219 | 0.574 | 6.789 | 2/2 |
| probes-4k-cellcull-live | 6.2198 | 0.0087 | 6.2010 | 0.0188 | -4.6306 | 0.573 | 6.789 | 2/2 |
| probes-4k-cell2-live | 7.1360 | 0.0042 | 7.1090 | 0.0270 | -3.7145 | 0.658 | 6.788 | 2/2 |
| probes-4k-frag2-live | 10.2636 | 0.0311 | 10.2636 | 0.0000 | -0.5869 | 0.946 | 6.863 | 2/2 |
| probes-4k-free-live | 6.4761 | 0.0020 | 6.4761 | 0.0000 | -4.3744 | 0.597 | 6.784 | 2/2 |
| probes-4k-frag-r1 | 9.1198 | 0.0975 | 9.1198 | 0.0000 | 0.0000 | 1.000 | 6.814 | 2/2 |
| probes-4k-cell-r1 | 6.2621 | 0.0901 | 6.2402 | 0.0219 | -2.8577 | 0.687 | 6.828 | 2/2 |
| probes-4k-cellcull-r1 | 6.2211 | 0.0196 | 6.2058 | 0.0153 | -2.8987 | 0.682 | 6.795 | 2/2 |
| probes-4k-cell2-r1 | 7.1333 | 0.0012 | 7.1115 | 0.0218 | -1.9865 | 0.782 | 6.786 | 2/2 |
| probes-4k-frag2-r1 | 8.7018 | 0.0064 | 8.7018 | 0.0000 | -0.4181 | 0.954 | 6.870 | 2/2 |
| far-1080p-frag-live | 0.5652 | 0.0002 | 0.5652 | 0.0000 | 0.0000 | 1.000 | 2.547 | 2/2 |
| far-1080p-cell-live | 0.8135 | 0.0007 | 0.4429 | 0.3706 | 0.2483 | 1.439 | 2.550 | 2/2 |
| far-1080p-cellcull-live | 0.8357 | 0.0006 | 0.4424 | 0.3933 | 0.2706 | 1.479 | 2.551 | 2/2 |
| far-1080p-cell2-live | 0.9348 | 0.0002 | 0.5639 | 0.3710 | 0.3697 | 1.654 | 2.548 | 2/2 |
| far-1080p-frag2-live | 0.5991 | 0.0022 | 0.5991 | 0.0000 | 0.0339 | 1.060 | 2.569 | 2/2 |
| far-1080p-free-live | 0.4189 | 0.0005 | 0.4189 | 0.0000 | -0.1463 | 0.741 | 2.576 | 2/2 |
| far-1080p-frag-r1 | 0.5684 | 0.0006 | 0.5684 | 0.0000 | 0.0000 | 1.000 | 2.548 | 2/2 |
| far-1080p-cell-r1 | 0.7685 | 0.0009 | 0.4425 | 0.3260 | 0.2000 | 1.352 | 2.555 | 2/2 |
| far-1080p-cellcull-r1 | 0.7923 | 0.0000 | 0.4427 | 0.3496 | 0.2239 | 1.394 | 2.550 | 2/2 |
| far-1080p-cell2-r1 | 0.8889 | 0.0024 | 0.5641 | 0.3248 | 0.3205 | 1.564 | 2.549 | 2/2 |
| far-1080p-frag2-r1 | 0.5919 | 0.0032 | 0.5919 | 0.0000 | 0.0234 | 1.041 | 2.558 | 2/2 |
| far-4k-frag-live | 1.9148 | 0.0031 | 1.9148 | 0.0000 | 0.0000 | 1.000 | 9.042 | 2/2 |
| far-4k-cell-live | 1.9590 | 0.0109 | 1.4926 | 0.4664 | 0.0442 | 1.023 | 9.130 | 2/2 |
| far-4k-cellcull-live | 1.9903 | 0.0054 | 1.4903 | 0.4999 | 0.0755 | 1.039 | 9.130 | 2/2 |
| far-4k-cell2-live | 2.2286 | 0.0023 | 1.7628 | 0.4658 | 0.3138 | 1.164 | 9.142 | 2/2 |
| far-4k-frag2-live | 1.7337 | 0.0073 | 1.7337 | 0.0000 | -0.1811 | 0.905 | 9.243 | 2/2 |
| far-4k-free-live | 1.3767 | 0.0075 | 1.3767 | 0.0000 | -0.5381 | 0.719 | 9.047 | 2/2 |
| far-4k-frag-r1 | 1.9138 | 0.0029 | 1.9138 | 0.0000 | 0.0000 | 1.000 | 9.042 | 2/2 |
| far-4k-cell-r1 | 1.9092 | 0.0294 | 1.4933 | 0.4159 | -0.0045 | 0.998 | 9.087 | 2/2 |
| far-4k-cellcull-r1 | 1.9398 | 0.0060 | 1.4954 | 0.4443 | 0.0260 | 1.014 | 9.141 | 2/2 |
| far-4k-cell2-r1 | 2.1816 | 0.0065 | 1.7672 | 0.4144 | 0.2678 | 1.140 | 9.145 | 2/2 |
| far-4k-frag2-r1 | 1.7269 | 0.0004 | 1.7269 | 0.0000 | -0.1869 | 0.902 | 9.346 | 2/2 |

## Cells and pixels per view (untraced count captures, cell buffer + cull)

`visible fluid px` is the fluid pass's occlusion-query sample count (every fluid draw, overdraw counted); `cells` is every cell of the dispatched faces' rects; `cells (cull)` those the frustum cull kept.

| view | atlas | atlas bytes | faces | cells | cells (cull) | visible fluid px | px/cell | px/cell (cull) |
|---|---|---|---|---|---|---|---|---|
| near-4k | 1344x1472 | 15826944 | 1 | 65536 | 20078 | 8294400 | 126.56 | 413.11 |
| near-1080p | 1344x1472 | 15826944 | 1 | 65536 | 20078 | 2073600 | 31.64 | 103.28 |
| probes-4k | 512x640 | 2621440 | 1 | 36864 | 12844 | 8294400 | 225.00 | 645.78 |
| far-4k | 1344x1472 | 15826944 | 10 | 1409024 | 1338209 | 998584 | 0.71 | 0.75 |
| far-1080p | 1344x1472 | 15826944 | 10 | 1409024 | 1338209 | 249942 | 0.18 | 0.19 |

## Per run

| run | cost ms (p10-p90) | fluid / cell ms | basis | forward ms | GPU frame ms complete/incomplete | frames complete/steady/total | clock MHz | temp C | valid |
|---|---|---|---|---|---|---|---|---|---|
| far-1080p-cell-live-r1 | 0.8138 | 0.4431 / 0.3707 | timestamps | 2.550 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-cell-r1-r1 | 0.7689 | 0.4425 / 0.3264 | timestamps | 2.561 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-cell2-live-r1 | 0.9347 | 0.5644 / 0.3704 | timestamps | 2.549 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-cell2-r1-r1 | 0.8877 | 0.5629 / 0.3248 | timestamps | 2.547 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-cellcull-live-r1 | 0.8360 | 0.4425 / 0.3936 | timestamps | 2.550 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-cellcull-r1-r1 | 0.7923 | 0.4426 / 0.3497 | timestamps | 2.548 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-frag-live-r1 | 0.5653 | 0.5653 / 0.0000 | timestamps | 2.547 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-frag-r1-r1 | 0.5687 | 0.5687 / 0.0000 | timestamps | 2.547 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-frag2-live-r1 | 0.5980 | 0.5980 / 0.0000 | timestamps | 2.574 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-frag2-r1-r1 | 0.5902 | 0.5902 / 0.0000 | timestamps | 2.548 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-free-live-r1 | 0.4192 | 0.4192 / 0.0000 | timestamps | 2.606 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-cell-live-r1 | 1.9644 | 1.4978 / 0.4666 | timestamps | 9.136 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-cell-r1-r1 | 1.8945 | 1.4819 / 0.4126 | timestamps | 9.025 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-cell2-live-r1 | 2.2298 | 1.7638 / 0.4660 | timestamps | 9.149 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-cell2-r1-r1 | 2.1849 | 1.7703 / 0.4146 | timestamps | 9.141 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-cellcull-live-r1 | 1.9930 | 1.4927 / 0.5002 | timestamps | 9.139 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-cellcull-r1-r1 | 1.9368 | 1.4931 / 0.4437 | timestamps | 9.142 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-frag-live-r1 | 1.9163 | 1.9163 / 0.0000 | timestamps | 9.057 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-frag-r1-r1 | 1.9152 | 1.9152 / 0.0000 | timestamps | 9.046 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-frag2-live-r1 | 1.7300 | 1.7300 / 0.0000 | timestamps | 9.245 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-frag2-r1-r1 | 1.7271 | 1.7271 / 0.0000 | timestamps | 9.247 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-free-live-r1 | 1.3804 | 1.3804 / 0.0000 | timestamps | 9.055 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-cell-live-r1 | 5.3362 | 5.3069 / 0.0293 | timestamps | 2.161 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-cell-r1-r1 | 5.3298 | 5.3022 / 0.0276 | timestamps | 2.159 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-cell2-live-r1 | 3.5074 | 3.4782 / 0.0292 | timestamps | 2.157 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-cell2-r1-r1 | 3.5003 | 3.4727 / 0.0275 | timestamps | 2.150 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-cellcull-live-r1 | 5.3234 | 5.3030 / 0.0205 | timestamps | 2.163 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-cellcull-r1-r1 | 5.3185 | 5.2991 / 0.0195 | timestamps | 2.162 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-frag-live-r1 | 5.7524 | 5.7524 / 0.0000 | timestamps | 2.172 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-frag-r1-r1 | 5.2795 | 5.2795 / 0.0000 | timestamps | 2.159 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-frag2-live-r1 | 3.9239 | 3.9239 / 0.0000 | timestamps | 2.178 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-frag2-r1-r1 | 3.8460 | 3.8460 / 0.0000 | timestamps | 2.168 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-free-live-r1 | 3.1155 | 3.1155 / 0.0000 | timestamps | 2.151 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-cell-live-r1 | 20.6865 | 20.6556 / 0.0309 | timestamps | 8.403 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-cell-r1-r1 | 21.2448 | 21.2159 / 0.0289 | timestamps | 8.311 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-cell2-live-r1 | 12.9050 | 12.8743 / 0.0307 | timestamps | 8.365 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-cell2-r1-r1 | 12.7004 | 12.6714 / 0.0290 | timestamps | 8.219 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-cellcull-live-r1 | 20.7221 | 20.7002 / 0.0219 | timestamps | 8.443 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-cellcull-r1-r1 | 20.4700 | 20.4493 / 0.0207 | timestamps | 8.325 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-frag-live-r1 | 22.3470 | 22.3470 / 0.0000 | timestamps | 8.416 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-frag-r1-r1 | 20.6605 | 20.6605 / 0.0000 | timestamps | 8.301 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-frag2-live-r1 | 15.0178 | 15.0178 / 0.0000 | timestamps | 8.280 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-frag2-r1-r1 | 14.6644 | 14.6644 / 0.0000 | timestamps | 8.305 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-free-live-r1 | 12.2394 | 12.2394 / 0.0000 | timestamps | 8.113 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-cell-live-r1 | 6.2295 | 6.2025 / 0.0270 | timestamps | 6.793 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-cell-r1-r1 | 6.3072 | 6.2853 / 0.0219 | timestamps | 6.876 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-cell2-live-r1 | 7.1339 | 7.1070 / 0.0269 | timestamps | 6.793 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-cell2-r1-r1 | 7.1327 | 7.1109 / 0.0218 | timestamps | 6.792 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-cellcull-live-r1 | 6.2242 | 6.2054 / 0.0188 | timestamps | 6.794 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-cellcull-r1-r1 | 6.2309 | 6.2155 / 0.0154 | timestamps | 6.805 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-frag-live-r1 | 10.8310 | 10.8310 / 0.0000 | timestamps | 6.786 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-frag-r1-r1 | 9.1686 | 9.1686 / 0.0000 | timestamps | 6.841 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-frag2-live-r1 | 10.2791 | 10.2791 / 0.0000 | timestamps | 6.854 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-frag2-r1-r1 | 8.6986 | 8.6986 / 0.0000 | timestamps | 6.865 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-free-live-r1 | 6.4771 | 6.4771 / 0.0000 | timestamps | 6.788 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-cell-live-r2 | 0.8131 | 0.4427 / 0.3704 | timestamps | 2.550 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-cell-r1-r2 | 0.7680 | 0.4425 / 0.3255 | timestamps | 2.548 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-cell2-live-r2 | 0.9350 | 0.5634 / 0.3715 | timestamps | 2.547 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-cell2-r1-r2 | 0.8901 | 0.5654 / 0.3247 | timestamps | 2.551 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-cellcull-live-r2 | 0.8355 | 0.4424 / 0.3931 | timestamps | 2.551 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-cellcull-r1-r2 | 0.7923 | 0.4429 / 0.3495 | timestamps | 2.551 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-frag-live-r2 | 0.5651 | 0.5651 / 0.0000 | timestamps | 2.546 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-frag-r1-r2 | 0.5681 | 0.5681 / 0.0000 | timestamps | 2.548 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-frag2-live-r2 | 0.6002 | 0.6002 / 0.0000 | timestamps | 2.565 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-frag2-r1-r2 | 0.5935 | 0.5935 / 0.0000 | timestamps | 2.569 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-1080p-free-live-r2 | 0.4187 | 0.4187 / 0.0000 | timestamps | 2.546 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-cell-live-r2 | 1.9535 | 1.4874 / 0.4661 | timestamps | 9.124 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-cell-r1-r2 | 1.9239 | 1.5046 / 0.4193 | timestamps | 9.149 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-cell2-live-r2 | 2.2274 | 1.7619 / 0.4655 | timestamps | 9.135 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-cell2-r1-r2 | 2.1783 | 1.7641 / 0.4142 | timestamps | 9.149 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-cellcull-live-r2 | 1.9875 | 1.4879 / 0.4996 | timestamps | 9.121 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-cellcull-r1-r2 | 1.9428 | 1.4978 / 0.4450 | timestamps | 9.141 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-frag-live-r2 | 1.9132 | 1.9132 / 0.0000 | timestamps | 9.028 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-frag-r1-r2 | 1.9123 | 1.9123 / 0.0000 | timestamps | 9.037 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-frag2-live-r2 | 1.7373 | 1.7373 / 0.0000 | timestamps | 9.242 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-frag2-r1-r2 | 1.7267 | 1.7267 / 0.0000 | timestamps | 9.445 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| far-4k-free-live-r2 | 1.3729 | 1.3729 / 0.0000 | timestamps | 9.039 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-cell-live-r2 | 5.3147 | 5.2855 / 0.0292 | timestamps | 2.151 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-cell-r1-r2 | 5.3187 | 5.2911 / 0.0276 | timestamps | 2.153 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-cell2-live-r2 | 3.5009 | 3.4717 / 0.0292 | timestamps | 2.147 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-cell2-r1-r2 | 3.5091 | 3.4815 / 0.0276 | timestamps | 2.155 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-cellcull-live-r2 | 5.3114 | 5.2910 / 0.0204 | timestamps | 2.154 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-cellcull-r1-r2 | 5.3075 | 5.2881 / 0.0194 | timestamps | 2.158 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-frag-live-r2 | 5.7160 | 5.7160 / 0.0000 | timestamps | 2.158 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-frag-r1-r2 | 5.2530 | 5.2530 / 0.0000 | timestamps | 2.150 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-frag2-live-r2 | 3.9782 | 3.9782 / 0.0000 | timestamps | 2.222 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-frag2-r1-r2 | 4.0187 | 4.0187 / 0.0000 | timestamps | 2.230 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-1080p-free-live-r2 | 3.1045 | 3.1045 / 0.0000 | timestamps | 2.142 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-cell-live-r2 | 20.5517 | 20.5209 / 0.0307 | timestamps | 8.346 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-cell-r1-r2 | 21.2001 | 21.1710 / 0.0291 | timestamps | 8.664 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-cell2-live-r2 | 12.8289 | 12.7984 / 0.0306 | timestamps | 8.254 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-cell2-r1-r2 | 12.6208 | 12.5918 / 0.0290 | timestamps | 8.173 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-cellcull-live-r2 | 20.4710 | 20.4493 / 0.0217 | timestamps | 8.336 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-cellcull-r1-r2 | 20.3742 | 20.3535 / 0.0207 | timestamps | 8.281 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-frag-live-r2 | 22.1288 | 22.1288 / 0.0000 | timestamps | 8.350 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-frag-r1-r2 | 20.6390 | 20.6390 / 0.0000 | timestamps | 8.335 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-frag2-live-r2 | 14.9197 | 14.9197 / 0.0000 | timestamps | 8.299 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-frag2-r1-r2 | 14.6464 | 14.6464 / 0.0000 | timestamps | 8.297 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| near-4k-free-live-r2 | 12.2454 | 12.2454 / 0.0000 | timestamps | 8.122 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-cell-live-r2 | 6.2276 | 6.2005 / 0.0270 | timestamps | 6.786 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-cell-r1-r2 | 6.2170 | 6.1952 / 0.0219 | timestamps | 6.780 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-cell2-live-r2 | 7.1381 | 7.1111 / 0.0270 | timestamps | 6.782 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-cell2-r1-r2 | 7.1339 | 7.1121 / 0.0218 | timestamps | 6.781 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-cellcull-live-r2 | 6.2155 | 6.1967 / 0.0188 | timestamps | 6.785 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-cellcull-r1-r2 | 6.2113 | 6.1962 / 0.0152 | timestamps | 6.785 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-frag-live-r2 | 10.8700 | 10.8700 / 0.0000 | timestamps | 6.799 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-frag-r1-r2 | 9.0711 | 9.0711 / 0.0000 | timestamps | 6.786 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-frag2-live-r2 | 10.2481 | 10.2481 / 0.0000 | timestamps | 6.872 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-frag2-r1-r2 | 8.7049 | 8.7049 / 0.0000 | timestamps | 6.874 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
| probes-4k-free-live-r2 | 6.4751 | 6.4751 / 0.0000 | timestamps | 6.781 | n/a/n/a | n/a/n/a/n/a | n/a | n/a | yes |
