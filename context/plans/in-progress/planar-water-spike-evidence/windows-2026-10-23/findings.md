# Windows planar reflection measurements — 2026-10-23

GTX 1660 SUPER: planar is a useful alternative to SSR High for the large lake, especially at higher scene resolutions. Half scale is near SSR Medium's synchronized completion at 1080p/4K, with different image quality. Tight D/R bounds are cheaper but visibly sacrifice reflections. These results do not select a shipping preset.

**Owner clarification, 2026-10-23:** distant reflection deliberately belongs to probes. Earlier references to lost detail describe the planar-to-probe handoff, not a failure to meet the image goal. The design favors planar plus probes on large water to avoid SSR's dependence on main-screen coverage. Judge transition quality and near-field usefulness; do not require the bounded hybrid to reproduce unlimited planar. A follow-up bounded live release experiment is recorded in `../windows-bounded-live-2026-10-23/`.

## Method and provenance

Branch `claude/planar-water-spike`, engine revision `1ff80ed97`. Dev profile, `dev-tools,capture`, `CARGO_INCREMENTAL=0`, indirect validation forced on. No engine or shader edits during this measurement session. NVIDIA driver 621.01, 6 GiB board VRAM. Exact binary, compiler, map/source hashes and inherited diagnostics: `windows-manifest.json`; each suite also has a manifest. No builds, bakes, game or Steam workload ran alongside captures. Desktop GPU use is not zero; machine-state and five-second telemetry samples are retained in the CSV files.

The lake was rebuilt from committed source before measuring; the old Windows PRL predated the Mac fixture edits. e24 and reflection-probes-test were rebuilt too. Default warm development bakes explicitly warn that bounded-reach indirect SH is approximate. They are not shipping/cold bakes. Windows lake PRL is 84,508,363 bytes, SHA-256 `7cce7c5f69397b20f2cab3740deecd97b5114d5f8985b737244752315b5f15f4`; it is not byte-identical to the Mac bake. Compare modes within each machine's dataset, not Mac/Windows absolute times as a controlled hardware ratio.

Main backend: Vulkan. Separate DX12 runs use the same binary, scene and validation policy. Each capture has 120 warmup and 240 measured frames, two complete 120-readback GPU windows, sync-proof SH preload and deterministic 1/60 animation steps. Repetition 2 reverses configuration/pose order. Scene extents are explicit: 960×540, 1920×1080 or 3840×2160; captures have no window/surface or render-resolution divisor. Planar scales are per axis: quarter targets have 1/16 of scene pixels, half targets 1/4.

**144 accepted captures, 34,560 measured frames:** lake primary 60, resolution 20, fixtures 24, bounds 8, long bounds 4, tier controls 12, DX12 6, high-coverage 4K 10. All have complete GPU windows, no malformed readbacks, expected SSR/planar pass counts and clean visible lightmap miss/read/install counters. Original scenes, reports, logs and PNGs are retained.

Completion below is the per-run median CPU+GPU completion with a device wait after every sample submit. It is not live gameplay frame pacing or FPS. GPU figures are timestamp means, weighted by sampled readbacks, then averaged across the two equal-sized runs. `planar_reflection` spans cull through resolve; `planar_forward` lies inside it and must not be added again. Absent passes retain null times and zero sampled readbacks.

Capture does not run the game VM or advance moving entities/camera. It prepares mirror visibility once, freezes receiver poses, and advances renderer animation only. Its CPU stages cover recording, not a per-frame live mirror walk or live submit. Capture fluid appearance also omits the manifest's fog, drift and wave tuning.

## Large lake: resolution

Across-water eye (-16.8656, 1.7, -48.768), yaw 90°, pitch −10°. Planar rows have unlimited D/R and 4 m fade bands. Each completion cell lists the two run medians in execution-repeat order. GPU cells average both runs.

| Mode | 1080p completion, ms | 1080p reflection GPU, ms | 4K completion, ms | 4K reflection GPU, ms |
|---|---:|---:|---:|---:|
| SSR High | 6.866 / 6.636 | 1.429 | 18.318 / 18.300 | 5.339 |
| SSR Medium | 5.516 / 5.484 | 0.317 | 14.020 / 14.000 | 1.130 |
| Planar half | 5.648 / 5.720 | 0.285 | 13.846 / 13.700 | 0.734 |
| Planar quarter | 5.683 / 5.580 | 0.157 | 13.248 / 13.212 | 0.295 |
| Planar full | 6.251 / 6.198 | 0.725 | 15.643 / 15.416 | 2.278 |

At 4K across water, half planar lowers reflection GPU time by about 86% and synchronized completion by about 25% against High. Quarter lowers completion by about 28%. At 1080p, half lowers completion by about 16% against High. Medium remains a strong cheaper baseline; the small half-versus-Medium completion difference is not a universal planar win.

At 960×540, half planar takes 3.374/3.188 ms versus High 3.384/3.738 and Medium 2.993/2.940. Its reflection span is 0.148 ms versus 0.426 High and 0.106 Medium. Fixed CPU/driver/wait costs matter more here. Quarter saves GPU time but small completion differences are noisy. Keep both repetitions: across R15 run 2 reaches 4.476 ms and quarter/D15/R15 run 1 reaches 3.507 ms; neither is discarded.

Water compositing is a separate cost. Across at 4K it stays about 1.32–1.38 ms across these modes, even when quarter planar's reflection span is only 0.295 ms. Faster reflection generation does not remove that floor.

### High water coverage at 4K

Eye (-28.8656, 1.7, -48.768), yaw 90°, pitch −55°. These are matched half/quarter/full controls, with D/R unlimited.

| Mode | Completion medians, ms | Reflection GPU mean, ms |
|---|---:|---:|
| SSR High | 24.474 / 24.673 | 9.134 |
| SSR Medium | 17.035 / 17.231 | 1.816 |
| Planar half | 18.064 / 18.260 | 2.347 |
| Planar quarter | 16.552 / 16.517 | 0.758 |
| Planar full | 23.796 / 23.927 | 8.091 |

Full scale nearly gives up the completion advantage against High here. Half is slower than Medium; quarter is modestly faster. Water compositing alone remains about 3.75–3.81 ms. Choose scale against coverage and quality, not the across-water pose alone.

## D, R and fade levers

The 60-run primary matrix covers scale quarter/half/full; D and R independently unlimited/30/15/8 m; quarter+D15+R15; half+D30+R15; and 0.5/4/8 m fade bands. Additional bounds cover D45/D60 and D80/D100. Full values, water-composite times and mirror-forward times are in [tables.md](tables.md); per-run details and counts are in `summary.json` and the original scene/report/log files.

At 960 across, half D15 reduces reflection GPU from 0.148 to 0.085 ms, with completion 3.064/3.075 ms. D8 buys essentially no additional reflection savings. R8 reaches 0.078 ms across, but downward it barely changes reflection cost (0.278 versus unlimited 0.280 ms): water near the downward eye is already inside R. Fade widths show no reliable speed advantage and are primarily a quality control.

| Across setting | Seeds | Drawable / reachable cells | Walk steps | Path |
|---|---:|---:|---:|---|
| Unlimited | 41 | 124 / 124 | 20000 | Step-limit fallback |
| D30 | 41 | 28 / 28 | 5091 | Seeded |
| D15 | 41 | 9 / 9 | 339 | Seeded |
| D8 | 41 | 8 / 8 | 98 | Seeded |
| R8 | 2 | 79 / 79 | 17188 | Seeded |

These are preparation counts from the actual integrated captures, not live CPU durations. D is the more effective CPU work bound in this lake. Mask coverage percentages and draw-call counts are not exported by the current measurement surface and remain unmeasured.

D80/D100 both return to 124 cells and the 20,000-step fallback. Across reflection spans are 0.123/0.148 ms respectively, with completion D80 3.318/3.154 ms and D100 3.244/3.260. Inspected D80/D100 images: D100 largely restores the unlimited image, while D80 still shows probe fallback. D100 also gives back the GPU and visibility-work savings. No tested finite bound preserves all the useful quality while retaining the aggressive D15 savings.

## Smaller fixtures and mirror-only residency

Each fixture has across-water and downward poses, matched High/Medium/half-planar controls and reverse-order repeats at 960×540. Half planar completion: e24 across 2.264/2.399 ms, down 2.306/2.291; probes across 2.587/2.562, down 2.684/2.699. Medium is faster in all four views (about 2.05–2.37 ms). High's completion is close to planar in several of these small views; do not generalize the 4K lake gain to all water.

Unlike the lake, these poses exercise additional mirror-only cells:

| Pose | Mirror-visible cells | Mirror-only cells | Walk steps |
|---|---:|---:|---:|
| e24 across | 29 | 4 | 1577 |
| e24 down | 20 | 18 | 233 |
| probes across | 29 | 9 | 4554 |
| probes down | 18 | 16 | 2302 |

All are seeded paths. Visible lightmap misses outside the baked set, visible nonresident misses, failed installs and failed reads are zero. This adds integrated mirror-only residency evidence beyond the automated seam tests. These static fixtures do not establish moving skinned-character or mover quality.

## Tier and backend controls

With planar disabled, Off and Low record neither SSR nor planar; Very High/Ultra run SSR, with reflection GPU means about 0.795/1.158 ms at the across lake pose. Enabling planar at Off still records no mirror pass. Enabling it at Low records half planar, about 0.149 ms, confirming the spike override applies to every non-Off tier.

DX12 at 960×540 across: High completion 4.338/4.354 ms, Medium 3.890/3.927, half planar 4.332/4.325. Reflection GPU means are in `tables.md`. There is no meaningful High-to-planar completion win here, despite lower reflection GPU work. Do not carry the Vulkan result across backends. The DX12 captures also exercise level-installed planar pipelines beyond the full-init FXC test.

Vulkan logs contain startup loader errors for installed Overwolf/ReShade layers that fail to load or are explicitly skipped. They are retained; these are not renderer validation failures. The experiment did not uninstall or change system overlays. No `Validation Error`, mirror `SolidCellFallback` or malformed timestamp readback occurs in accepted reports.

## Live CPU and submission

Four 20-second Vulkan launches, High → half planar → half planar → High. Full dev mod/VM, ordinary streaming, indirect validation on, windowed vsync; committed surface and scene both 1600×900, divisor 1. Start capsule centre (-16.8656, 1.2, -48.768), yaw 90°, pitch −10°; gravity settles the centre near y=1.0 and the descriptor's eye near y=1.5. This differs from the static capture's eye height and fluid presentation. Saved player reflection quality was High.

`live/summary.json` averages the last three complete 120-frame CPU windows per launch (360 frames each), excluding early load/settle work. GPU summaries use the last complete 120-readback window per launch; GPU logs round to 0.01 ms and do not align with CPU windows. The initial live launch had only two GPU windows, so one tail GPU window is used consistently in all four runs. No clocks were locked; the live machine-state sample is retained. These are CPU-cost probes, not an uncapped FPS benchmark.

| Launch order | Work CPU, ms | Mirror walk, ms | Main portal walk, ms | Render submit, ms | Total frame, ms |
|---|---:|---:|---:|---:|---:|
| High 1 | 6.193 | absent | 3.192 | 1.007 | 16.527 |
| Planar 2 | 9.255 | 2.976 | 3.142 | 1.216 | 16.530 |
| Planar 3 | 9.204 | 2.915 | 3.169 | 1.202 | 16.528 |
| High 4 | 5.996 | absent | 3.161 | 0.989 | 16.527 |

The unlimited mirror walk reaches the 20,000-step fallback every frame and adds roughly **3 ms CPU work per frame**. Submission also rises about 0.2 ms. Tail reflection GPU work is lower (planar 0.38/0.37 ms versus High 1.41/1.35), but every launch remains vsync-limited near 60 Hz. The static capture gain is not proof of a live FPS win. Bounded live walk timing remains unmeasured: D controls are capture fields/dev sliders, and no extra runtime launch knobs were added for this session. The dramatic D15 walk-count reduction motivates that next measurement.

Accepted live logs contain no main/mirror SolidCellFallback or renderer validation error; planar runs record no SSR. `live-invalid-start/` is excluded: an initial capsule-centre y=0 placed the body inside the deck, after which it fell away from the target view. Those logs remain for provenance; the corrected script starts above the deck. Every launched engine process was stopped after its bounded run.

## Visual evidence

Inspected lake High, half, quarter, D15, R8 and D60, the down half view, and both smaller across-water fixtures. Half retains recognizable walls, blocks and thin posts; quarter makes reflection boundaries coarser. D15 exposes stretched/smeared probe fallback, and R8 removes much of the distant-water planar image. D60 still loses large-room reflections; a finite bound cannot be chosen from speed alone. The down view is dominated by the bed and a nearby post, so it is useful for coverage cost but weak for distant-reflection quality.

Useful image comparisons:

- [Lake High](lake-sweep/across-ssr-high-run1.png), [half](lake-sweep/across-planar-half-run1.png), [quarter](lake-sweep/across-planar-quarter-run1.png).
- [D15 fallback](lake-sweep/across-d-15-run1.png), [R8 fallback](lake-sweep/across-r-8-run1.png), [D60](bounds/across-d60-run1.png).
- [e24](fixtures/e24-across-planar-half-run1.png), [probe room](fixtures/probes-across-planar-half-run1.png).

Still images and static live camera probes do not prove grazing-motion stability, temporal fade quality or owner visual acceptance. No pixel-parity claim against SSR is made; it has different failure modes and coverage.

## Verification and replay

128 focused checks passed: CPU planar 7, visibility mirror 5, renderer planar 8 (six adapter-backed proofs, no adapter skips), existing fluid pins 100, FXC full-init 1 explicitly executed with `--ignored`, app/capture planar 7. Logs are kept here. No engine changes followed these checks; only measurement scripts and notes changed.

Run from the spike workspace root after building/baking per the handoff:

```powershell
$env:WGPU_BACKEND='vulkan'
python context/plans/in-progress/planar-water-spike-evidence/knob-sweep/run_sweep.py --output target/new-lake-sweep
python context/plans/in-progress/planar-water-spike-evidence/windows-2026-10-23/run_followups.py --suite resolution --output target/new-resolution-sweep
```

Other follow-up suites: `fixtures`, `bounds`, `long-bounds`, `tiers`, `dx12`, `coverage`. All require a fresh output directory. Saved scene files reproduce the exact pose/settings; change output/report paths when replaying. `summarize.py` validates saved suite directories here and regenerates tables plus compact JSON, including per-pass counts, recording CPU stages, lightmap pool bytes and mirror diagnostics. Do not run timing beside another capture, bake or build.

`run_live.ps1` performs the bounded live sequence and requires a fresh `live/` directory. It opens a visible window because hidden/background rendering suspends. `summarize_live.py` validates and reduces the live tail windows. The focused test commands and build/bake instructions remain in the Windows handoff and per-test logs.

## Decision at this checkpoint (superseded by bounded live follow-up)

Keep planar dev-only. Half or quarter with D/R unlimited has promising desktop GPU cost, but the live lake's extra ~3 ms CPU walk prevents recommending it as-is. Bound or improve that visibility work, then measure live again. Quarter is a quality tradeoff, not a free optimization. Avoid full scale as the default and avoid selecting D15/R8 solely for speed. Keep SSR Medium as a serious alternative for small views and other hardware/backends. The Mac completion penalty remains a separate unresolved result, not disproven by this Windows gain. Bounded live timing, moving-camera/entity checks and owner acceptance remain required before shipping.

**Follow-up completed:** [release bounded live findings](../windows-bounded-live-2026-10-23/findings.md) measure 16 uncapped runs. Half D15 reduces mirror CPU to 0.061 ms and frame time to 5.070 ms versus unlimited 7.830 and High 4.828 ms. The owner's distant probe handoff is intentional. Half D15 is now the first motion-test candidate; quarter D15's static image equals probes-only at the tested pose, so speed alone cannot establish its usefulness. Moving-camera/entity and near-water scale checks remain open.
