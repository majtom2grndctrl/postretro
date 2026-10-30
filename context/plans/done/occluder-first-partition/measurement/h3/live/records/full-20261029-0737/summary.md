# Live H3 batch `full-20261029-0737`

```
started=2026-10-29 07:37:29 head=30a01746f dirty=0
model=MacBookPro16,1 macos=26.6.2 build=25G83
bin 9164ab4a6432 /Users/dhiester/Projects/Personal/postretro/target/occluder-h3live/bin/postretro
bin bee2f97ea04f /Users/dhiester/Projects/Personal/postretro/target/occluder-h3live/bin/scripts-build
bin 7992dd951ea2 /Users/dhiester/Projects/Personal/postretro/target/occluder-h3live/bin/walktime-capped
prl before occlusion-test 138360125 d43c362635dd
prl before stress-street 129785845 6a7bc2c960f5
prl cap26 occlusion-test 117452197 bd16e3db5404
prl cap26 stress-street 130326049 e850ddda2dc7
prl cap19 occlusion-test 114958149 2a88336384b8
prl cap19 stress-street 134987341 8ae1e4c0774a
pmset:  lowpowermode 0; powernap 1; displaysleep 20 (display sleep prevented by caffeinate, caffeinate); sleep 1 (sleep prevented by caffeinate, caffeinate, caffeinate, caffeinate, caffeinate, Claude);
displays:  Chipset Model: Intel UHD Graphics 630; Chipset Model: AMD Radeon Pro 5300M; Resolution: 3072 x 1920 Retina; Main Display: Yes;
state: 2026-10-29 07:37:31 util=0 inUseVidMemoryBytes=568942592 vramFreeBytes=2835546112 mhz=0 temp=63 | cpu_speed_limit=100 cpu_sched_limit=100 cpus=16 thermal_warn=none perf_warn=none xcpm_thermal=52 screensaver=no locked=absent
```

Sentinels and flags (from `status`):

```
07:38:14 SENTINEL batch-start: sentinel01 total_ms=16.521 rc=0 2026-10-29 07:38:14 util=0 inUseVidMemoryBytes=570839040 vramFreeBytes=2835546112 mhz=0 temp=62 | cpu_speed_limit=100 cpu_sched_limit=100 cpus=16 thermal_warn=none perf_warn=none xcpm_thermal=19 screensaver=no locked=absent
08:04:02 SENTINEL periodic: sentinel02 total_ms=16.522 rc=0 2026-10-29 08:04:02 util=0 inUseVidMemoryBytes=582930432 vramFreeBytes=2785099776 mhz=0 temp=63 | cpu_speed_limit=100 cpu_sched_limit=100 cpus=16 thermal_warn=none perf_warn=none xcpm_thermal=55 screensaver=no locked=absent
08:30:58 SENTINEL periodic: sentinel03 total_ms=16.506 rc=0 2026-10-29 08:30:57 util=0 inUseVidMemoryBytes=578318336 vramFreeBytes=2753245184 mhz=0 temp=62 | cpu_speed_limit=100 cpu_sched_limit=100 cpus=16 thermal_warn=none perf_warn=none xcpm_thermal=72 screensaver=no locked=absent
08:57:19 SENTINEL periodic: sentinel04 total_ms=16.518 rc=0 2026-10-29 08:57:19 util=0 inUseVidMemoryBytes=588091392 vramFreeBytes=2750468096 mhz=0 temp=63 | cpu_speed_limit=100 cpu_sched_limit=100 cpus=16 thermal_warn=none perf_warn=none xcpm_thermal=66 screensaver=no locked=absent
09:23:30 SENTINEL periodic: sentinel05 total_ms=16.507 rc=0 2026-10-29 09:23:30 util=0 inUseVidMemoryBytes=583069696 vramFreeBytes=2746175488 mhz=0 temp=62 | cpu_speed_limit=100 cpu_sched_limit=100 cpus=16 thermal_warn=none perf_warn=none xcpm_thermal=70 screensaver=no locked=absent
09:51:02 SENTINEL closing: sentinel06 total_ms=16.514 rc=0 2026-10-29 09:51:02 util=0 inUseVidMemoryBytes=579145728 vramFreeBytes=2745356288 mhz=0 temp=63 | cpu_speed_limit=100 cpu_sched_limit=100 cpus=16 thermal_warn=none perf_warn=none xcpm_thermal=84 screensaver=no locked=absent
```

Render extents (first run): surface=2560x1440 scene=1280x720 divisor=2

## Per run

ms unless noted; CPU = mean over kept windows; GPU = trace busy fraction × frame total; MHz / °C = AMD median clock and max temp while tracing; idle = before the run.

| pose | side | verdict | gate | total | work | wait_acq | walk | tests | µs/test | trip frames | submit | record | GPU/frame | GPU gfx+cmp | MHz | °C trace | °C idle | VRAM idle GiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| occlusion-test `fc:P1` r1 | before | OK | GATE_OK | 16.63 | 8.99 | 7.57 | 0.607 | 2023 | 0.300 | 0 | 5.94 | 1.59 | 12.69 | 11.51 | 980 | 100 | 57 | 0.53 |
| occlusion-test `fc:P1` r1 | cap26 | OK | GATE_OK | 16.55 | 7.09 | 9.38 | 0.099 | 310 | 0.320 | 0 | 4.64 | 1.52 | 12.71 | 11.54 | 868 | 96 | 58 | 0.53 |
| occlusion-test `fc:P1` r1 | cap19 | OK | GATE_OK | 16.55 | 7.19 | 9.28 | 0.135 | 407 | 0.332 | 0 | 4.73 | 1.48 | 12.57 | 11.40 | 896 | 96 | 58 | 0.54 |
| occlusion-test `fc:P2` r1 | cap26 | OK | GATE_OK | 16.55 | 7.03 | 9.44 | 0.114 | 377 | 0.301 | 0 | 4.61 | 1.50 | 12.43 | 11.25 | 847 | 95 | 58 | 0.55 |
| occlusion-test `fc:P2` r1 | cap19 | OK | GATE_OK | 16.55 | 7.16 | 9.32 | 0.152 | 483 | 0.314 | 0 | 4.71 | 1.47 | 12.59 | 11.38 | 868 | 96 | 57 | 0.54 |
| occlusion-test `fc:P2` r1 | before | OK | GATE_OK | 16.56 | 8.95 | 7.53 | 0.622 | 2031 | 0.306 | 0 | 5.85 | 1.61 | 13.06 | 11.87 | 847 | 96 | 58 | 0.54 |
| occlusion-test `fc:G1` r1 | cap19 | OK | GATE_OK | 16.54 | 7.06 | 9.41 | 0.029 | 68 | 0.433 | 0 | 4.77 | 1.40 | 11.47 | 9.62 | 588 | 94 | 58 | 0.54 |
| occlusion-test `fc:G1` r1 | before | OK | GATE_OK | 16.54 | 8.26 | 8.20 | 0.405 | 1262 | 0.321 | 0 | 5.57 | 1.43 | 11.40 | 10.44 | 490 | 94 | 58 | 0.54 |
| occlusion-test `fc:G1` r1 | cap26 | OK | GATE_OK | 16.54 | 6.93 | 9.54 | 0.024 | 58 | 0.411 | 0 | 4.62 | 1.46 | 11.41 | 10.44 | 537 | 93 | 57 | 0.54 |
| occlusion-test `fc:G2` r1 | before | OK | GATE_OK | 16.59 | 8.76 | 7.75 | 0.489 | 1595 | 0.307 | 0 | 5.85 | 1.58 | 13.93 | 11.49 | 1004 | 101 | 56 | 0.54 |
| occlusion-test `fc:G2` r1 | cap26 | OK | GATE_OK | 16.55 | 6.99 | 9.49 | 0.049 | 131 | 0.377 | 0 | 4.72 | 1.43 | 13.23 | 12.08 | 989 | 96 | 57 | 0.54 |
| occlusion-test `fc:G2` r1 | cap19 | OK | GATE_OK | 16.54 | 7.04 | 9.43 | 0.061 | 165 | 0.369 | 0 | 4.79 | 1.40 | 13.12 | 11.97 | 1004 | 97 | 58 | 0.54 |
| occlusion-test `fc:G3` r1 | cap26 | OK | GATE_OK | 16.55 | 7.10 | 9.38 | 0.029 | 68 | 0.420 | 0 | 4.76 | 1.50 | 11.31 | 10.41 | 468 | 93 | 58 | 0.53 |
| occlusion-test `fc:G3` r1 | cap19 | OK | GATE_OK | 16.55 | 6.93 | 9.54 | 0.027 | 63 | 0.422 | 0 | 4.77 | 1.35 | 11.35 | 10.48 | 503 | 95 | 58 | 0.52 |
| occlusion-test `fc:G3` r1 | before | OK | GATE_OK | 16.54 | 7.50 | 8.96 | 0.168 | 530 | 0.316 | 0 | 5.10 | 1.41 | 11.27 | 10.36 | 526 | 93 | 57 | 0.54 |
| occlusion-test `fc:L2` r1 | cap19 | OK | GATE_OK | 16.53 | 7.23 | 9.23 | 0.074 | 223 | 0.332 | 0 | 4.92 | 1.43 | 11.22 | 10.33 | 470 | 95 | 58 | 0.58 |
| occlusion-test `fc:L2` r1 | before | OK | GATE_OK | 16.54 | 8.32 | 8.15 | 0.394 | 1279 | 0.308 | 0 | 5.63 | 1.46 | 11.22 | 10.35 | 453 | 93 | 58 | 0.54 |
| occlusion-test `fc:L2` r1 | cap26 | OK | GATE_OK | 16.55 | 7.27 | 9.20 | 0.067 | 209 | 0.322 | 0 | 4.90 | 1.50 | 11.22 | 10.33 | 484 | 94 | 58 | 0.54 |
| occlusion-test `fc:C1` r1 | before | OK | GATE_OK | 16.55 | 6.06 | 10.41 | 0.006 | 9 | 0.658 | 0 | 4.42 | 0.94 | 10.46 | 8.90 | 402 | 94 | 56 | 0.54 |
| occlusion-test `fc:C1` r1 | cap26 | OK | GATE_OK | 16.54 | 5.95 | 10.51 | 0.006 | 9 | 0.705 | 0 | 4.34 | 0.93 | 10.90 | 10.18 | 400 | 95 | 58 | 0.54 |
| occlusion-test `fc:C1` r1 | cap19 | OK | GATE_OK | 16.54 | 5.94 | 10.52 | 0.006 | 9 | 0.709 | 0 | 4.32 | 0.92 | 11.07 | 10.31 | 408 | 95 | 58 | 0.54 |
| occlusion-test `fc:C2` r1 | cap26 | OK | GATE_OK | 16.54 | 6.83 | 9.63 | 0.013 | 25 | 0.535 | 0 | 4.50 | 1.46 | 11.69 | 9.60 | 610 | 94 | 57 | 0.54 |
| occlusion-test `fc:C2` r1 | cap19 | OK | GATE_OK | 16.53 | 6.54 | 9.92 | 0.011 | 20 | 0.561 | 0 | 4.39 | 1.31 | 11.78 | 10.86 | 564 | 95 | 58 | 0.54 |
| occlusion-test `fc:C2` r1 | before | OK | GATE_OK | 16.53 | 6.78 | 9.67 | 0.016 | 33 | 0.488 | 0 | 4.61 | 1.30 | 11.50 | 10.61 | 564 | 93 | 57 | 0.54 |
| occlusion-test `fc:V1` r1 | cap19 | OK | GATE_OK | 16.56 | 8.18 | 8.31 | 0.162 | 518 | 0.314 | 0 | 6.10 | 1.11 | 11.67 | 10.77 | 598 | 95 | 58 | 0.54 |
| occlusion-test `fc:V1` r1 | before | OK | GATE_OK | 16.55 | 8.25 | 8.23 | 0.506 | 1652 | 0.306 | 0 | 5.81 | 1.12 | 10.76 | 2.93 | 651 | 93 | 58 | 0.54 |
| occlusion-test `fc:V1` r1 | cap26 | OK | GATE_OK | 16.56 | 8.15 | 8.33 | 0.142 | 442 | 0.322 | 0 | 6.06 | 1.13 | 12.17 | 11.23 | 534 | 94 | 58 | 0.54 |
| stress-street `rd:plaza` r1 | before | OK | GATE_OK | 16.77 | 12.06 | 4.64 | 0.948 | 3577 | 0.265 | 0 | 7.15 | 1.93 | 11.74 | 11.05 | 400 | 94 | 56 | 0.54 |
| stress-street `rd:plaza` r1 | cap26 | OK | GATE_OK | 16.98 | 14.96 | 1.95 | 0.824 | 3330 | 0.247 | 0 | 7.92 | 2.13 | 11.19 | 10.48 | 372 | 95 | 58 | 0.54 |
| stress-street `rd:plaza` r1 | cap19 | OK | GATE_OK | 16.87 | 14.45 | 2.35 | 0.857 | 3364 | 0.255 | 0 | 7.82 | 2.22 | 11.00 | 10.29 | 371 | 92 | 57 | 0.54 |
| stress-street `rd:promenade` r1 | cap26 | OK | GATE_OK | 16.89 | 14.28 | 2.53 | 0.563 | 1936 | 0.291 | 0 | 9.58 | 1.70 | 10.90 | 10.30 | 356 | 95 | 58 | 0.54 |
| stress-street `rd:promenade` r1 | cap19 | OK | GATE_OK | 16.91 | 14.42 | 2.41 | 0.580 | 1964 | 0.295 | 0 | 9.68 | 1.70 | 10.84 | 10.29 | 370 | 92 | 58 | 0.54 |
| stress-street `rd:promenade` r1 | before | OK | GATE_OK | 16.59 | 10.24 | 6.27 | 0.516 | 1867 | 0.276 | 0 | 6.67 | 1.43 | 11.06 | 10.57 | 380 | 92 | 58 | 0.54 |
| stress-street `rd:elevated` r1 | cap19 | OK | GATE_OK | 16.99 | 15.27 | 1.64 | 0.761 | 2620 | 0.290 | 0 | 10.64 | 1.69 | 10.94 | 10.32 | 432 | 92 | 58 | 0.54 |
| stress-street `rd:elevated` r1 | before | OK | GATE_OK | 16.59 | 10.96 | 5.55 | 0.721 | 2762 | 0.261 | 0 | 7.22 | 1.44 | 11.06 | 10.57 | 428 | 93 | 58 | 0.55 |
| stress-street `rd:elevated` r1 | cap26 | OK | GATE_OK | 17.00 | 15.09 | 1.83 | 0.750 | 2577 | 0.291 | 0 | 10.33 | 1.73 | 11.01 | 10.46 | 424 | 92 | 58 | 0.54 |
| stress-street `rd:upperdeck` r1 | before | OK | GATE_OK | 16.61 | 10.41 | 6.12 | 0.996 | 3793 | 0.263 | 0 | 6.92 | 1.22 | 11.16 | 10.70 | 442 | 95 | 57 | 0.54 |
| stress-street `rd:upperdeck` r1 | cap26 | OK | GATE_OK | 16.58 | 10.57 | 5.94 | 0.466 | 1795 | 0.260 | 0 | 7.25 | 1.28 | 11.13 | 9.09 | 451 | 93 | 58 | 0.55 |
| stress-street `rd:upperdeck` r1 | cap19 | OK | GATE_OK | 16.59 | 10.45 | 6.06 | 0.447 | 1802 | 0.248 | 0 | 7.13 | 1.27 | 11.27 | 10.81 | 454 | 94 | 57 | 0.54 |
| stress-street `wk:g11.26.1@0` r1 | cap26 | OK | GATE_OK | 17.01 | 15.45 | 1.48 | 1.075 | 4156 | 0.259 | 0 | 8.71 | 2.24 | 10.70 | 10.02 | 375 | 92 | 58 | 0.54 |
| stress-street `wk:g11.26.1@0` r1 | cap19 | OK | GATE_OK | 17.02 | 15.07 | 1.87 | 1.086 | 4153 | 0.261 | 0 | 8.69 | 2.25 | 10.87 | 10.41 | 376 | 94 | 58 | 0.54 |
| stress-street `wk:g11.26.1@0` r1 | before | OK | GATE_OK | 16.63 | 13.05 | 3.52 | 1.279 | 4812 | 0.266 | 0 | 7.86 | 1.89 | 10.95 | 10.31 | 393 | 92 | 57 | 0.54 |
| stress-street `wk:roof:A0.s0.e@0/-15` r1 | cap19 | OK | GATE_OK | 16.74 | 13.00 | 3.67 | 0.766 | 3478 | 0.220 | 0 | 7.79 | 2.06 | 10.22 | 9.68 | 423 | 93 | 58 | 0.54 |
| stress-street `wk:roof:A0.s0.e@0/-15` r1 | before | OK | GATE_OK | 16.62 | 11.89 | 4.66 | 0.950 | 3771 | 0.252 | 0 | 7.18 | 1.77 | 10.98 | 10.37 | 406 | 92 | 57 | 0.54 |
| stress-street `wk:roof:A0.s0.e@0/-15` r1 | cap26 | OK | GATE_OK | 16.71 | 13.15 | 3.49 | 0.735 | 3382 | 0.217 | 0 | 7.89 | 2.07 | 11.00 | 10.38 | 404 | 92 | 58 | 0.54 |

## Live walk factor

in-process = walktime-capped (dev) median over 3 repeats at the settled live eye (floor+1.31); factor = live µs/test ÷ in-process µs/test. Rows under 1,000 live tests are noisy (fixed per-walk overhead).

| pose | side | live tests | live µs/test | in-proc tests | in-proc µs/test | factor | in-proc tests at floor+1.5 |
|---|---|---|---|---|---|---|---|
| occlusion-test `fc:P1` | before | 2023 | 0.300 | 1968 | 0.171 | 1.75 | 2019 |
| occlusion-test `fc:P1` | cap26 | 310 | 0.320 | 302 | 0.148 | 2.16 | 301 |
| occlusion-test `fc:P1` | cap19 | 407 | 0.332 | 415 | 0.156 | 2.13 | 371 |
| occlusion-test `fc:P2` | cap26 | 377 | 0.301 | 377 | 0.150 | 2.01 | 360 |
| occlusion-test `fc:P2` | cap19 | 483 | 0.314 | 494 | 0.159 | 1.98 | 454 |
| occlusion-test `fc:P2` | before | 2031 | 0.306 | 2016 | 0.175 | 1.75 | 2030 |
| occlusion-test `fc:G1` | cap19 | 68 | 0.433 | 68 | 0.150 | 2.89 | 68 |
| occlusion-test `fc:G1` | before | 1262 | 0.321 | 1232 | 0.181 | 1.77 | 1251 |
| occlusion-test `fc:G1` | cap26 | 58 | 0.411 | 58 | 0.145 | 2.84 | 58 |
| occlusion-test `fc:G2` | before | 1595 | 0.307 | 1595 | 0.175 | 1.76 | 1595 |
| occlusion-test `fc:G2` | cap26 | 131 | 0.377 | 131 | 0.154 | 2.44 | 131 |
| occlusion-test `fc:G2` | cap19 | 165 | 0.369 | 165 | 0.150 | 2.45 | 165 |
| occlusion-test `fc:G3` | cap26 | 68 | 0.420 | 55 | 0.149 | 2.82 | 68 |
| occlusion-test `fc:G3` | cap19 | 63 | 0.422 | 51 | 0.149 | 2.83 | 63 |
| occlusion-test `fc:G3` | before | 530 | 0.316 | 284 | 0.155 | 2.05 | 515 |
| occlusion-test `fc:L2` | cap19 | 223 | 0.332 | 247 | 0.147 | 2.26 | 223 |
| occlusion-test `fc:L2` | before | 1279 | 0.308 | 1199 | 0.174 | 1.77 | 1278 |
| occlusion-test `fc:L2` | cap26 | 209 | 0.322 | 237 | 0.143 | 2.25 | 209 |
| occlusion-test `fc:C1` | before | 9 | 0.658 | 9 | 0.140 | 4.70 | 9 |
| occlusion-test `fc:C1` | cap26 | 9 | 0.705 | 9 | 0.158 | 4.47 | 9 |
| occlusion-test `fc:C1` | cap19 | 9 | 0.709 | 9 | 0.159 | 4.46 | 9 |
| occlusion-test `fc:C2` | cap26 | 25 | 0.535 | 25 | 0.144 | 3.72 | 25 |
| occlusion-test `fc:C2` | cap19 | 20 | 0.561 | 20 | 0.143 | 3.94 | 20 |
| occlusion-test `fc:C2` | before | 33 | 0.488 | 33 | 0.160 | 3.05 | 33 |
| occlusion-test `fc:V1` | cap19 | 518 | 0.314 | 495 | 0.167 | 1.88 | 518 |
| occlusion-test `fc:V1` | before | 1652 | 0.306 | 1641 | 0.167 | 1.83 | 1648 |
| occlusion-test `fc:V1` | cap26 | 442 | 0.322 | 419 | 0.164 | 1.96 | 442 |
| stress-street `rd:plaza` | before | 3577 | 0.265 | 3563 | 0.148 | 1.80 | 3585 |
| stress-street `rd:plaza` | cap26 | 3330 | 0.247 | 3329 | 0.138 | 1.79 | 3363 |
| stress-street `rd:plaza` | cap19 | 3364 | 0.255 | 3363 | 0.140 | 1.82 | 3408 |
| stress-street `rd:promenade` | cap26 | 1936 | 0.291 | 1967 | 0.156 | 1.87 | 1962 |
| stress-street `rd:promenade` | cap19 | 1964 | 0.295 | 2236 | 0.162 | 1.83 | 2232 |
| stress-street `rd:promenade` | before | 1867 | 0.276 | 1840 | 0.155 | 1.78 | 1925 |
| stress-street `rd:elevated` | cap19 | 2620 | 0.290 | 2620 | 0.169 | 1.72 | 2690 |
| stress-street `rd:elevated` | before | 2762 | 0.261 | 2696 | 0.157 | 1.66 | 2939 |
| stress-street `rd:elevated` | cap26 | 2577 | 0.291 | 2338 | 0.168 | 1.73 | 2453 |
| stress-street `rd:upperdeck` | before | 3793 | 0.263 | 3731 | 0.156 | 1.69 | 3771 |
| stress-street `rd:upperdeck` | cap26 | 1795 | 0.260 | 1795 | 0.146 | 1.78 | 1800 |
| stress-street `rd:upperdeck` | cap19 | 1802 | 0.248 | 1802 | 0.141 | 1.76 | 1806 |
| stress-street `wk:g11.26.1@0` | cap26 | 4156 | 0.259 | 2882 | 0.127 | 2.04 | 4164 |
| stress-street `wk:g11.26.1@0` | cap19 | 4153 | 0.261 | 3086 | 0.130 | 2.01 | 4147 |
| stress-street `wk:g11.26.1@0` | before | 4812 | 0.266 | 4726 | 0.154 | 1.73 | 4083 |
| stress-street `wk:roof:A0.s0.e@0/-15` | cap19 | 3478 | 0.220 | 3496 | 0.119 | 1.85 | 3524 |
| stress-street `wk:roof:A0.s0.e@0/-15` | before | 3771 | 0.252 | 3711 | 0.142 | 1.78 | 3917 |
| stress-street `wk:roof:A0.s0.e@0/-15` | cap26 | 3382 | 0.217 | 3396 | 0.115 | 1.89 | 3436 |

Factor over runs with ≥ 1,000 live tests: median 1.78, range 1.66–2.04 (n=24). research.md / H3 estimate: ≈ 1.4 (1.3–1.7).
