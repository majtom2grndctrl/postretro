# Glossy world reflections, review fixes (HEAD 080e5d9d0): re-measure and parity

The owner-approved trimmed batch after checkpoint 4's review fixes: water's reflecting shader back to the pre-branch up-only search and five-lobe ratio, the floor march's mip, stride-back hit and 32-byte uniform, the overlay's multi-plane capture-shadow tap, and the mover guard's own timing pair (26). Four items: (1) e24 Fluid Surface Pass, `8dda8a34a` against HEAD; (2) the floor-only fixture at High, overlay and march against the ceilings; (3) the guard pass on the mover fixture; (4) row 1 and G1 parity.

**Result: the GPU timing items (1–3) did not run.** The 5300M was in the latched slow regime (`memory: Mac GPU slowdown investigation`; `rendering_pipeline.md` §12) from the pilot's first run, and a cool-down and three re-probes (the last 2 h 17 min after onset) left it there. No timing figure below is a measurement of the code. Item 4 (CPU only) ran in full: row 1 passes (280 probes over the 11 water maps) and G1 holds on all 11 (one fixture-path difference in a script section, shown byte-identical once masked).

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

Scratch (not kept): this session's scratchpad holds `after/` (HEAD binaries), `hg/` and `hn/` (HEAD bakes with and without the flag), `sec/` and `logs/` (section digests, id 54 bodies, filtered `-v` logs), `row1/after/` (the row 1 after inputs), `bake.py`, `g1.py`, `g1.json`, `g1.out`, `row1.log`, `meas.sh`, `cool1.log`, `slow-regime-1/pilot.log`, `regime-summaries/` (the probes' kit tables). Before inputs: Task 12b's `t12/rb`, `t12/sec/rb`, `t12/logs/rb`, `t12/row1/before`.
