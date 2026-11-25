# Glossy world reflections, review fixes (HEAD 080e5d9d0): re-measure and parity

The owner-approved trimmed batch after checkpoint 4's review fixes: water's reflecting shader back to the pre-branch up-only search and five-lobe ratio, the floor march's mip, stride-back hit and 32-byte uniform, the overlay's multi-plane capture-shadow tap, and the mover guard's own timing pair (26). Four items: (1) e24 Fluid Surface Pass, `8dda8a34a` against HEAD; (2) the floor-only fixture at High, overlay and march against the ceilings; (3) the guard pass on the mover fixture; (4) row 1 and G1 parity.

**Result (Run 2, after a reboot, 24/24 runs valid with the memory-clock guard): items 1–3 measured.** (1) The e24 Fluid Surface Pass +4 % is gone: before / HEAD 5.861 / 5.870 ms at 1440p, 12.900 / 12.931 ms at 4K (+0.2 %, inside the runs' spread). (2) Seven of eight floor-only ceilings pass; overlay across at 4K is 4.527 ms against 4.52, 0.007 ms (0.15 %) over in all three runs: a tripwire breach, reported. (3) The guard pass costs 0.090 ms at 1440p and 0.053 ms at 4K. See "Run 2 (after reboot)".

**First attempt: the GPU timing items (1–3) did not run.** The 5300M was in the latched slow regime (`memory: Mac GPU slowdown investigation`; `rendering_pipeline.md` §12) from the pilot's first run, and a cool-down and three re-probes (the last 2 h 17 min after onset) left it there. No timing figure below is a measurement of the code. Item 4 (CPU only) ran in full: row 1 passes (280 probes over the 11 water maps) and G1 holds on all 11 (one fixture-path difference in a script section, shown byte-identical once masked).

## Binaries and bakes

- **Before:** Task 12b's pre-branch binaries (`postretro` with `capture,dev-tools`, `prl-build`, and a `scripts-build` beside them) in the Task 12b session scratchpad's `before/`. Provenance: every earlier results file records them as the `8dda8a34a` build (`results-glossy-task12-parity.md`, `-task12-m6.md`, `-task11-flooronly.md`, `-task10-flooronly.md`, `-task6.md`); their embedded source paths name that session's `wt-before` worktree; `prl-build --help` has no `--glossy-reflections` and the engine has no `Glossy` pass labels, so they predate the branch; `8dda8a34a` is the branch's merge-base with `main`. Not rebuilt.
- **After:** release build of HEAD `080e5d9d0` (clean tree), `postretro` with `capture,dev-tools`, `prl-build`, `scripts-build`, copied out of `target/release`. Its engine carries the `Glossy Mover Guard Pass` label.
- **Before bakes (reused, not rebaked):** Task 12b's fresh `--no-cache` bakes of the 11 water maps by the `8dda8a34a` compiler (`t12/bake.py`, variant `rb`: `prl-build <map> --no-cache --no-tui -j 6 -v --baked-root <own root>/baked`), with their filtered `-v` logs. That compiler has not changed, so a rebake would reproduce them; before reuse, every section of each of the 11 PRLs on disk was re-hashed and matched the digests recorded at bake time (all 11 match, id 54 bytes included).
- **After bakes (fresh, this session):** HEAD's `prl-build`, the same command, each map into its own copy of `content/dev` (maps copied, every other entry symlinked): the 11 water maps with `--glossy-reflections` (row 1 and G1) and without it (G1); for timing, e24 with the flag, and `metal/maps/glossy-floor-only.map` (as `glossy-reflections-test.map`) and `metal/maps/glossy-floor-mover.map` with the flag. The floor-only bake places 2 probes and 7 world bindings on 1 plane (7 up-facing faces), as at checkpoint 4.

## Items 1–3: not measured (slow regime)

Batch as planned (`meas.sh`, Task 12b's driver form): one interleaved arm list, mask 17F, 300 + 300 frames, `XCTRACE_GRACE=900`, `MAP_ROOT_BEFORE` = the reused pre-branch bakes, `MAP_ROOT_AFTER` = HEAD's flag bakes. Arms: `near-{1440,4k}:{before,after}:17F` (item 1); `glossy-{across,down}-{1440,4k}:after:17F:high` (item 2); `glossy-mover-across-{1440,4k}:after:17F:high` (item 3). Pilot first: 1 round, `--abort-invalid`.

Machine state before the pilot: owner idle about 1.5 h (HID idle), screen unlocked, no screen saver, nothing building, `caffeinate -dimsu` held until 02:45 and 05:57 (two holders) plus the kit's own; GPU 72–76 °C idle, the same as 12b's run starts (72–73 °C); `pmset -g therm` no thermal or performance warning; uptime 12 days; Claude desktop, a VPN client and a VM running.

The pilot's first run was the regime, and every later run stayed in it:

| run (UTC) | arm | frames | GPU frame ms | fluid ms | forward ms | overlay ms | march ms | busy core clock MHz | busy memory clock MHz, median (mode) | GPU °C | healthy reference |
|---|---|---|---|---|---|---|---|---|---|---|---|
| pilot r1, 06:44 | e24 near 1440p, before | 300 + 300 | 277.808 | 72.795 | 112.752 | — | — | 1232 | 324 (192) | 69–74 | 12b M6 same arm: frame 27.683, fluid 5.859, forward 14.056; memory 994 |
| re-probe, 07:07 (after 15 min idle) | e24 near 1440p, before | 120 + 120 | 278.212 | 72.710 | 113.122 | — | — | 1232 | 350 (192) | 68–82 | as above |
| re-probe 3, 09:01 (after 2 h of CPU-only bakes) | e24 near 1440p, before | 120 + 120 | 278.353 | 72.419 | 113.218 | — | — | 1232 | 192 (192) | 61–68 | as above |
| re-probe 2, 07:11 | glossy-across 1440p High, HEAD | 120 + 120 | 144.585 | — | 94.581 | 26.291 | 18.039 | 1231 | 254 (192) | 70–76 | checkpoint 4 same arm: frame 16.632, overlay 2.049, march 1.612, forward 11.127; memory 994 |

- **The signature is the memory clock.** Under load it sat at about 192 MHz in every slow run (busy-sample medians 254–350 MHz), where 12b's M6 and checkpoint 4's runs held a median of 994 MHz. The core clock was at its ceiling (1231–1232 MHz, higher than the healthy runs' 1050–1170), GPU activity 99%, temperature normal. Every pass slowed by roughly 8–13×, both binaries alike, so the cause is the machine, not either build.
- **Cool-down did not clear it:** 15 minutes idle, GPU 72 → 61 °C (log `cool1.log`), then the re-probe was unchanged to 0.2%; a third probe 2 h 17 min after onset (after item 4's bakes) was unchanged too. This matches the 2026-10-28 episode (flat while the GPU cooled, cleared only by a reboot), down to the long uptime and the desktop apps.
- The kit marked all four runs valid: its complete-frame and machine-state checks do not test the memory clock. The pilot was stopped by hand after its first run; nothing past it ran. Re-probe 3 is listed beside the other e24 rows, out of time order.
- **Next step for the owner:** a reboot (an agent cannot), ideally with only the terminal open, then rerun items 1–3 with `meas.sh pilot` and `meas.sh main` from the scratchpad below; bakes and binaries are in place. A pre-flight probe worth adding to the kit: fail a run whose busy memory-clock median is under ~900 MHz on this machine.

Records: `metal/runs/glossy-review-fixes-{pilot,reprobe,reprobe2,reprobe3}-*` (scene, report, run, gpu, trace status). The kit's summary tables for the three re-probes were moved to scratch, not kept as results files.

## Run 2 (after reboot): items 1–3

2026-11-25, 03:32–05:05 PST (builds, bakes, pilot, main), uptime 7 min at start (the owner rebooted the Mac to clear the regime). Same arms, frames and driver form as planned above; everything rebuilt and rebaked, since the reboot wiped the scratchpad.

### Binaries and bakes

- **Before:** `8dda8a34a` built fresh in a disposable worktree with its own target dir (`CARGO_INCREMENTAL=0`, release, `postretro` with `capture,dev-tools`, then `prl-build` and `scripts-build`), copied out; worktree and target deleted. Its `prl-build --help` has no `--glossy-reflections`, its engine no `Glossy Mover Guard Pass` label.
- **After:** HEAD `a89729576`, release, same packages and features. No code change since `080e5d9d0` (`git diff 080e5d9d0 HEAD -- crates shaders tools content Cargo.*` is empty), so this measures the review-fix build.
- **Bakes (fresh, `--no-cache --no-tui -j 6 -v`, each binary's own compiler):** e24 by `8dda8a34a` (with `8dda8a34a`'s own copy of `content/dev` beside it, maps aside); e24 by HEAD with `--glossy-reflections`; `metal/maps/glossy-floor-only.map` (as `glossy-reflections-test.map`) and `metal/maps/glossy-floor-mover.map` by HEAD with `--glossy-reflections` (7 up-facing glossy faces on 1 plane, as at checkpoint 4). The e24 `.map` is byte-identical at both commits. `--baked-root` is `<root>/baked`, the content root's grandparent, which is where the engine looks for `.prm` sidecars.
- **Discarded first pilot:** the first pilot's bakes used `--baked-root <root>/content/dev/baked`, so every texture loaded as a placeholder (`cannot read …/baked/materials/*.prm`). The kit caught it on the glossy arm (no overlay or march drew: 0 complete frames); the two e24 runs before it had passed the kit's checks. Those three runs were deleted and every map rebaked; the second pilot's logs show no missing sidecar.

### Procedure and validity

`meas.py` (scratch) wraps `run_ablation.py` unchanged and adds the slow-regime guard: a run whose busy-sample (GPU activity ≥ 50 %) memory-clock median is under 900 MHz is invalid, so `--abort-invalid` stops on it. The guard reproduced the first attempt's four medians (324, 350, 254, 192 MHz) from their run records, and 12b's M6 and M7 runs read 994–1026 MHz.

```
CELL_BIN_BEFORE=<scratch>/before/postretro CELL_BIN_AFTER=<scratch>/after/postretro \
MAP_ROOT_BEFORE=<scratch>/hb/content/dev/maps MAP_ROOT_AFTER=<scratch>/ha/content/dev/maps XCTRACE_GRACE=900 \
python3 meas.py --tag glossy-review-fixes-run2 --report glossy --rounds 2 --warmup 300 --samples 300 --abort-invalid \
  --arms near-1440:before:17F,near-1440:after:17F,near-4k:before:17F,near-4k:after:17F,\
glossy-{across,down}-{1440,4k}:after:17F:high,glossy-mover-across-{1440,4k}:after:17F:high
```

- **Pilot** (`--rounds 1`; one run per config type, at 4K: e24 before, e24 after, floor-only across, mover across): 4/4 valid. **Main:** 2 interleaved rounds (forward, then reversed), 10 arms, 20/20 valid. Every run passed both the kit's checks (complete frames, screen not locked, no screen saver, no foreign build) and the memory-clock guard.
- **Memory clock:** busy-sample medians 994–1318 MHz across the 24 runs (mode 994 in every run but one 4K pilot run at 1354); none near the regime's 192. Busy core clock medians 1063–1219 MHz, as in 12b.
- **Machine:** GPU 66–84 °C over the runs (idle 71–75 °C between batches, as at 12b's starts); `pmset -g therm` no thermal or performance warning; `caffeinate -dimsu` held throughout plus the kit's own; nothing building during runs.
- Figures are per complete frame. With two runs per arm, median and mean agree; spread = max − min. Pilot figures are listed separately and not pooled.

### Item 1: e24 Fluid Surface Pass, `8dda8a34a` vs HEAD (same session)

| resolution | before ms (runs) | HEAD ms (runs) | Δ ms | Δ % | 12b M6 (cross-session): before → HEAD then |
|---|---|---|---|---|---|
| 1440p | 5.861 (5.855, 5.868) | 5.870 (5.875, 5.865) | +0.009 | +0.15 % | 5.859 → 6.087 (+0.228, +3.9 %) |
| 4K | 12.900 (12.900, 12.900) | 12.931 (12.902, 12.959) | +0.031 | +0.24 % | 12.869 → 13.421 (+0.552, +4.3 %) |

4K pilot pair: before 12.921, HEAD 12.907 (−0.014 ms). **Verdict: the +4 % is gone.** The remaining difference is smaller than HEAD's own run spread at 4K (0.057 ms) and changes sign in the pilot pair; 12b's gap was 7–16× its spreads in every pair. The before binary reproduces 12b's before figures within 0.03 ms.

Same runs, context: forward 14.085 → 14.161 ms at 1440p, 31.247 → 31.296 ms at 4K (+0.5 % / +0.2 %; 12b's M6: +1.1 % / +0.5 %); probe sums faster on HEAD (0.048 → 0.037, 0.058 → 0.036 ms); GPU frame +0.063 / +0.137 ms (12b: +0.42 / +0.75).

### Item 2: floor-only fixture at High, HEAD

| figure | pose | 1440p ms (runs) | ceiling | verdict | 4K ms (runs) | ceiling | verdict |
|---|---|---|---|---|---|---|---|
| overlay | across | 2.054 (2.055, 2.053) | 2.06 | pass (0.3 % under) | 4.527 (4.523, 4.532; pilot 4.527) | 4.52 | **breach, +0.007 ms (+0.15 %)** |
| overlay | down | 3.061 (3.062, 3.060) | 3.16 | pass | 6.817 (6.812, 6.822) | 7.05 | pass |
| march | across | 1.589 (1.594, 1.585) | 2.01 | pass | 3.548 (3.552, 3.545; pilot 3.549) | 4.52 | pass |
| march | down | 1.901 (1.906, 1.896) | 2.38 | pass | 4.153 (4.152, 4.155) | 5.17 | pass |

The 4K across breach is in all three runs (4.523–4.532 ms), so it is real rather than noise, though smaller than the cross-batch allowance the plan grants the floor-layer figures (0.1 ms). Reported per the tripwire rule; cause not investigated. Cross-session context only: checkpoint 4 measured this arm at 4.506 ms (+0.021 now) and across 1440p at 2.049 (+0.005). Overlay down (3.049 / 6.764 then) and both marches (1.612 / 3.618 across, 1.903 / 4.139 down then) are within ±0.05 ms of checkpoint 4.

Other figures, same runs: chain 0.368 / 0.505 ms across, 1.167 / 0.468 down (1440p / 4K); forward 11.143 / 25.026 across, 9.468 / 23.389 down; sums 0.014–0.019; GPU frame 16.615 / 35.908 across, 16.802 / 36.825 down. Down at 1440p splits forward and chain differently in its two runs (forward 9.060 / 9.877, chain 1.554 / 0.781) with an unchanged sum (10.614 / 10.658) and frame: the trace's forward/chain-level attribution on floor-layer frames (`results-glossy-task11-diag.md`), not a cost change.

### Item 3: guard pass on the mover fixture, HEAD at High

| resolution | Glossy Mover Guard Pass ms (runs) | first measurement |
|---|---|---|
| 1440p | 0.090 (0.111, 0.068) | yes; no ceiling |
| 4K | 0.053 (0.046, 0.061; pilot 0.045) | yes; no ceiling |

The guard costs about 0.05–0.1 ms, under 0.2 % of the frame. Odd but small: 1440p reads higher than 4K, with run spreads of 0.043 and 0.015 ms, about the size of the figure itself. At this size the cost is likely dominated by fixed cost and timing noise rather than per-pixel work; not investigated. Same runs, context: overlay 1.900 / 4.182, march 1.609 / 3.511, chain 0.310 / 0.534, Kinematic Brush Pass 2.657 / 5.951, forward 15.647 / 34.381, GPU frame 23.929 / 51.102 ms.

### Run 2 records

- Kit tables: `results-glossy-review-fixes-run2.{md,json}` (main, means), `results-glossy-review-fixes-run2-pilot.{md,json}`.
- Per run: `metal/runs/glossy-review-fixes-run2-*` and `metal/runs/glossy-review-fixes-run2-pilot-*` (scene, report, run (with `busy_mem_clock_mhz` and `temp_c_range`), gpu, trace status, log). The gzipped trace exports in `metal/raw/` were deleted once reduced.

## Item 4: row 1 and G1 parity

Ran after the timing attempt (CPU-heavy; the GPU was unusable anyway). Inputs: before = Task 12b's reused `8dda8a34a` bakes (above); after = this session's fresh HEAD bakes, all 11 `content/dev` water maps (campaign-test, e24, reflection-lake, reflection-probes, vault, the five mover-closet maps, stress-env-volumes) with and without `--glossy-reflections`. Bake wall time: 4–124 s per small map; stress-env-volumes 6403 s, both variants in parallel at `-j 6`.

### Row 1: water placement, tiles, shares and exclusion

```
POSTRETRO_WATER_PARITY_BEFORE_DIR=<Task 12b t12/row1/before> POSTRETRO_WATER_PARITY_AFTER_DIR=<row1/after: HEAD flag bakes + -v logs> \
  cargo test -p postretro-level-compiler --test reflection_probe_water_parity_ab -- --ignored
```

**Passed.** 280 probes over the 11 maps matched the build before (stress-env-volumes 256, e24 13, reflection-probes 3, eight maps 1 each); none binds a world face. Matched per probe: source entity, box, capture point, light rosters and references, +Y/±X/±Z shares bit-exact with −Y zero, mip-0 colour and distance tiles, and the log's excluded-brush count. Negative control, run once and discarded: vault-test's excluded-brush count edited in a copy of the after log fails on `excluded brushes`.

### G1: without the flag, id 54 as before and no world binding

Compare script `g1.py` (Task 12b's `t12/g1.py`, retargeted at these bakes): every section other than 54 byte-identical by SHA-256 with equal version and size; id 54 present in both or neither; where present, the pre-branch version-2 section and HEAD's version-3 section equal in header (tile size, border, atlas and distance formats, K), every probe record, every light's roster index and reference, the five water lobes' shares (HEAD's −Y share zero), fluid bindings, mip-0 colour and distance tiles; HEAD's section binds no world face and holds one colour mip. HEAD's id 54 wire format is unchanged since Task 12b (no `wire.rs` change since `f5b479b44`); the parser's whole-body length assertion held on all 22. Negative controls on reflection-probes' section: one flipped byte in a distance tile and one in probe 0's record each fail on that field.

| HEAD bake | maps | id 54 in both | id 54 as before, −Y zero, 1 colour mip, 0 world bindings | other sections identical |
|---|---|---|---|---|
| without `--glossy-reflections` | 11 | 11 | 11 | 10; stress-env-volumes: all but id 28, which matches once its source path is masked (below) |
| with `--glossy-reflections` | 11 | 11 | 11 | the same |

Probes per map (both bakes): stress-env-volumes 256 (382 fluid bindings, 68 stored light shares), e24 13 (18 bindings), reflection-probes 3 (8 bindings, 9 shares), reflection-lake 1 (33 bindings), the other seven 1 each.

**Id 28 (`DataScript`) on stress-env-volumes is a fixture artefact, not a compiler change.** The section embeds the level script's absolute source path (u32 length, then the path). Task 12b's map roots symlinked `stress-env-volumes.generated.ts` to the checkout (path `/Users/…/postretro/content/dev/maps/…`, 92 bytes); this session copied the file into each root, so HEAD's bakes name the scratch path (167 bytes), and the flag and flagless bakes differ from each other only in their root's name. With the length and path masked, the rest of the payload is byte-identical between the before bake and both HEAD bakes. The 10 other water maps have no level script. Task 12b's HEAD bakes of the same map matched the before bake on id 28 for the same reason (symlinked script). A rebake with the script symlinked (another 107 min) was not run.

The runtime half of G1 (no world-face probe resource, no overlay without the flag) is the GPU gates' (Tasks 4, 5, 10); not re-run here.


## Artefacts

Scratch (not kept): the first attempt's scratchpad (wiped by the reboot) held `after/` (HEAD binaries), `hg/` and `hn/` (HEAD bakes with and without the flag), `sec/` and `logs/` (section digests, id 54 bodies, filtered `-v` logs), `row1/after/` (the row 1 after inputs), `bake.py`, `g1.py`, `g1.json`, `g1.out`, `row1.log`, `meas.sh`, `cool1.log`, `slow-regime-1/pilot.log`, `regime-summaries/` (the probes' kit tables). Before inputs: Task 12b's `t12/rb`, `t12/sec/rb`, `t12/logs/rb`, `t12/row1/before`.

Run 2 scratch (not kept), under the session scratchpad's `meas2/`: `before/` and `after/` (binaries), `hb/` and `ha/` (map roots: maps, `baked/`, content), `logs/` (bake `-v` logs), `meas.py` (the guard wrapper), `memclk.py` (the guard applied to existing run records), `analyze.py` (per-run tables above), `pilot.log`, `main.log`.
