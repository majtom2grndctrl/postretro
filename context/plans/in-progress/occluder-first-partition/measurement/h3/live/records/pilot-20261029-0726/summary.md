# Live H3 batch `pilot-20261029-0726`

```
started=2026-10-29 07:26:23 head=30a01746f dirty=0
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
pmset:  lowpowermode 0; powernap 1; displaysleep 20 (display sleep prevented by caffeinate, caffeinate); sleep 1 (sleep prevented by Claude, caffeinate, caffeinate, caffeinate, caffeinate, caffeinate);
displays:  Chipset Model: Intel UHD Graphics 630; Chipset Model: AMD Radeon Pro 5300M; Resolution: 3072 x 1920 Retina; Main Display: Yes;
state: 2026-10-29 07:26:25 util=3 inUseVidMemoryBytes=522678272 vramFreeBytes=2945937408 mhz=0 temp=60 | cpu_speed_limit=100 cpu_sched_limit=100 cpus=16 thermal_warn=none perf_warn=none xcpm_thermal=10 screensaver=no locked=absent
```

Sentinels and flags (from `status`):

```
07:27:23 SENTINEL batch-start: sentinel01 total_ms=16.520 rc=0 2026-10-29 07:27:22 util=0 inUseVidMemoryBytes=580771840 vramFreeBytes=2898669568 mhz=0 temp=67 | cpu_speed_limit=100 cpu_sched_limit=100 cpus=16 thermal_warn=none perf_warn=none xcpm_thermal=27 screensaver=no locked=absent
07:37:14 SENTINEL closing: sentinel02 total_ms=16.521 rc=0 2026-10-29 07:37:13 util=0 inUseVidMemoryBytes=568766464 vramFreeBytes=2835546112 mhz=0 temp=63 | cpu_speed_limit=100 cpu_sched_limit=100 cpus=16 thermal_warn=none perf_warn=none xcpm_thermal=68 screensaver=no locked=absent
```

Render extents (first run): surface=2560x1440 scene=1280x720 divisor=2

## Per run

ms unless noted; CPU = mean over kept windows; GPU = trace busy fraction × frame total; MHz / °C = AMD median clock and max temp while tracing; idle = before the run.

| pose | side | verdict | gate | total | work | wait_acq | walk | tests | µs/test | trip frames | submit | record | GPU/frame | GPU gfx+cmp | MHz | °C trace | °C idle | VRAM idle GiB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| stress-street `rd:promenade` r1 | before | OK | GATE_OK | 16.59 | 10.27 | 6.24 | 0.522 | 1867 | 0.280 | 0 | 6.68 | 1.41 | 11.22 | 10.75 | 386 | 95 | 58 | 0.54 |
| stress-street `rd:promenade` r1 | cap26 | OK | GATE_OK | 16.83 | 14.18 | 2.57 | 0.555 | 1936 | 0.287 | 0 | 9.52 | 1.70 | 10.82 | 8.72 | 363 | 92 | 58 | 0.54 |
| stress-street `rd:promenade` r1 | cap19 | OK | GATE_OK | 16.89 | 14.36 | 2.45 | 0.581 | 1964 | 0.296 | 0 | 9.64 | 1.68 | 10.85 | 10.29 | 363 | 94 | 58 | 0.54 |
