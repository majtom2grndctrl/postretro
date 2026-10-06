# sh-compose-row-cost-spike — plan of record

mode: compact
status: active
read at: 42cec06d8

## Layout

- Worktree `../postretro-shspike`, branch `sh-compose-row-cost-spike`. It holds this plan, `measurements/sh-compose-row-cost-spike/`, and the findings note. It is the only branch that lands.
- Worktree `../postretro-shspike-probes`, branch `sh-compose-row-cost-spike-probes`, cut from this branch. It holds every shader, pipeline and instrumentation probe. It carries no `unsafe` (see the H-e correction). It is deleted unmerged when the spike lands.
- Builds use `CARGO_TARGET_DIR=/Users/dhiester/Projects/Personal/postretro/target`, so dependencies are shared with the main tree. Disk had 18 GB free at claim time, and a second release target would cost about 8 GB.
- The PRL fixtures are gitignored, so each worktree symlinks `content/dev/maps/*.prl` from the main tree.

## Corrections

- Brief says wgpu 29.0.1 / wgpu-hal 29 (H-e). Since `de735f026` the stack is **wgpu 30.0.1** (`1ad33de46`). `wgpu-hal-30.0.1/src/metal/device.rs` still picks `BoundsCheckPolicy::Restrict` unless runtime checks are off, and passes `force_loop_bounding` through. `Device::create_shader_module_trusted` is still the only way in, and it is still `unsafe`. The premise holds; only the version changes.
- Path: "`ShProbeReadback::encode_copy` already copies the total atlas." It copies only the indirect total atlas. It is also an async dev-tools overlay readback. The byte comparison needs Pass B's `direct_composed_atlas` as well. So the probe branch adds a synchronous capture-time dump of both composed `rgba16float` atlases instead.
- Fixture `campaign-test.prl` was re-baked on 2026-10-05. Its prefix is now `2470294d8cb54730`, not shadow-fill-cost's `574be03376fdb9cf`. The other two fixtures match shadow-fill-cost: hallway-inspection `a543d0e0291dbceb`, kinematic-platform `8939e012bc683c0c`. The compose shaders and their pipelines have not changed since `de735f026`.
- Arm selection: one release binary carries every arm. An environment variable (`POSTRETRO_SPIKE_ARMS`, a comma list, so arms stack) is read at pipeline creation and rewrites the WGSL source. The brief allows this ("by an environment variable read at pipeline creation"). All arms then share one build lineage, which removes a binary-layout confound between arms. (The planned `spike-trusted-compose` feature and `trusted` arm were never built; see below.)
- Each arm rewrites both compose shaders where the hypothesis applies to both. Metal System Trace labels the two passes separately, so one run yields both passes' deltas.
- **H-e `trusted` arm not built.** The session's auto-mode permission classifier refused the file containing the `unsafe` `create_shader_module_trusted` call site, as a security weakening, despite the brief's owner approval. It was not routed around. The static gate (task 2) ran, and the timed arm waits on the owner: either allow the edit, or rule H-e reported from the static gate plus the safe `const-tile` arm. The probe module carries no `trusted` arm and no `spike-trusted-compose` feature.
- **`const-tile` arm added** (safe, intended exact). The H-e gate found `naga_div`/`naga_mod` guards in every per-texel iteration. `read_delta_texel` divides by the runtime `grid.tile_dimension`, and PRL validation pins the tile to 6×6 RGB16F (`DELTA_TILE_TEXEL_F16_COUNT`), so the constant form is exact.
- **Worktree content.** Release builds need the gitignored compiled scripts (`content/dev/**/*.js`, `content/base/`). They are copied from the main tree, whose script sources match this branch. `baked/` is symlinked from the main tree. Batch `arena1` ran before the `baked/` link, so material textures used placeholders there. Compose passes never sample materials, but VRAM and bandwidth pressure differed from later batches, so `arena1` is preliminary and is never compared across batches.
- **The probes branch fails one guard by design.** The drift guard `renderer_direct_uploads_and_submissions_have_lifecycle_owners` fails on the probes branch because the capture atlas dump submits directly. The dump is spike-only, and the branch never lands.
- **Between-launch comparison replaced by in-launch pairs (H3 method).** Each launch lands in one of two or three memory regimes that scale the SH-atlas passes by up to ~1.2× (`diag1`: same binary and arm, 6.7 vs 8.1 ms). So `arena3`'s between-launch interleave was unreadable. With `POSTRETRO_SPIKE_ARMS_B`, each compose pass builds A and B pipelines that alternate per frame, and B passes are labelled `… [B]`. The A/A null pair reads within ±0.02 ms by median. This keeps the brief's interleaving rule, per frame, and removes the regime confound. The layout arm `coalesced-b` cannot pair, so it uses the Pass B ÷ indirect ratio, which is stable across regimes.
- **Added levers beyond the brief's candidates**, as the data warranted: `array-free` (+ `unroll36`, `l0-only`) and `coalesced-b`. They follow the brief's rule that a hypothesis whose ablation moves beyond spread gets an exact lever.
- **Shared target dir, shared binary path.** The probe release binary is written to the main tree's `target/release/postretro`. Each probe build is copied at once to `scratchpad/bin/probe-<commit>/`, and batches run that copy.

## Delegated answers

- Rounding-only lever (FMA contraction) — the note reports the lever's maximum absolute deviation over the dumped atlases and marks the recommendation conditional, per the brief.
- Pass B L1/L2 entry counts too small to fit — the note first adds poses whose per-level counts differ. Any term still unidentifiable is reported unfitted and named. The floor and skip-coarse arms then give a single-pose share as a cross-check.

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| H1 one commit + fixture set per batch, SHA-256 prefixes | `measurements/sh-compose-row-cost-spike/batches.json` (`manifest.py`): per batch, the probe commit and binary SHA-256 from every run record, plus fixture prefixes | achievable as stated | **pass**. Every batch ran one binary. Fixtures `a543d0e0291dbceb` / `2470294d8cb54730` / `8939e012bc683c0c`. No probe touches row planning, so no batch straddles a change to which rows compose. |
| H2 foreground/unlocked/no-saver per run, idle ioreg per batch, invalid count | `run.py` validity record, `runs/*-idle.ioreg.txt` | achievable as stated | **pass**. 215 valid runs (202 traced). 1 invalid (`arena2-baseline-r1`, screen locked; its batch stopped). 1 run never started (`arenaP`, label bug; batch rerun as `arenaQ`). |
| H3 ≥3 traces per arm per pose with spread; discard runs whose rows change | `summarize_paired.py` → `paired-*.json`; `rows_stable` in each run record | achievable as stated | **pass**. Every reported arm has 3 paired launches (5 for the `coalesced-b` ratio). No valid run changed its rows. **Correction:** between-launch spread is dominated by per-launch memory regimes, so deltas come from per-frame A/B pairs inside one launch (findings §Method notes). |
| H4 lever + stacked byte-identical to baseline (≥2 stepped times, streamed + full-resident), oracle passes | `capture.sh` + `compare_atlas.py` (12 scenes); oracle test with the arm env | achievable as stated | **pass** (see the oracle list under Tasks 6–7). Every lever and stack is byte-identical in 12/12 scenes, and the oracle passes on every lever and stack. Gap: there is no L1/L2 fixture for 27/45, because the compiler never emits one. |
| H5 negative control differs | `floor` through the same compare | achievable as stated | **pass**. `floor` differs in 12/12 scenes, in both atlases. |
| H6 no ablation/floor arm recommended; every arm typed | findings §Attribution and §Recommendations | achievable as stated | **pass**. Only `array-free+unroll36` is recommended. |
| F1 rows and entries by brick level per pose and pass | `[SH spike counts]`, `runs/counts-*` | achievable as stated | **reported**. All rows are L0 on every pose, with 0 L1/L2. Rows with entries and lane-entries are added. |
| F2 baseline per-pass ms at 4 poses; fitted model with spread | `model.json` (`fit_model.py`, bootstrap) | achievable as stated | **reported**. `c` = 0.24 / 0.27 ms; `r` = 2.98 / 3.45 µs per row. The per-entry term is not one constant, and the L1/L2 terms are unfitted. |
| F3 floor share per pass | paired `floor` | achievable as stated | **reported**. The entry share is 1.1–2.5% (indirect) and 3.6–5.4% (Pass B). |
| F4 H-a..H-e ablation deltas (+ lever where built) | findings §Attribution | achievable as stated | **reported**, with one gap. H-e's timed `trusted` arm was **not run**, because the permission classifier blocked the `unsafe` call site. H-e is reported from the static gate plus the safe `unroll36` / `const-tile` probes. |
| F5 remainder | findings §Top-down partition | achievable as stated | **reported** |
| F6 stacked saving vs sum of levers, arena + kinematic | paired `array-free+unroll36` | achievable as stated | **reported**. −2.23 / −2.89 ms (arena), −2.13 / −3.04 ms (station). |
| F7 shader statistics or "unavailable" with tools tried | findings §Shader statistics | achievable as stated | **unavailable**. Tools tried are named there. |
| F8 1660 Super per-pass ms | owner handoff | manual | **outstanding**. Handoff instructions are in findings §1660. |
| N1 findings note | `findings.md` | achievable as stated | **pass** |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Probe infrastructure on the probes branch: arm rewrite (`baseline`, `floor`) for both passes, streamed and legacy pipelines; per-level row/entry count log; capture atlas dump. Copy measurement tooling. Then the riskiest premise: baseline vs floor at the arena, 3 interleaved traces each. Does the per-pass spread resolve a split, and do the rows stay stable? | integrating executor | — | done. Probes `27c10a573`, `1ddd56691`; `every_arm_rewrites_and_validates` passes. Batch `arena1` (6 of 6 valid, rows stable at 2129 L0 rows / 110 L0 entries per pass): indirect 6.73 → floor 6.58 ms; Pass B 8.00 → 7.59 ms. Spread ≤ 0.15 ms, so the split resolves. The fixed per-row term is ~95–98% at this pose. |
| 2 | H-e static gate: emit both composed shaders' MSL with checks on and off (naga safe API), count checks inside the per-entry/per-texel loops | delegated (read-only + scratch test) | — | done. Checks sit inside the per-entry and per-texel loops in both shaders (`measurements/sh-compose-row-cost-spike/he-gate/REPORT.md`). |
| 3 | Atlas byte-compare harness (dump → compare, max abs deviation); negative control on `floor` | integrating executor | 1 | done. 12 scenes × (baseline, every arm); dumps of 3.6 GB per arm are compared and then deleted. |
| 4 | Baseline + floor at the 4 brief poses plus extra poses for the per-level fit; F1 counts; model fit | integrating executor | 1 | done. Batches `arenaQ`, `campaignP`, `kinspawnP`, `kinmidP`; `model.json`. No pose has L1/L2 rows, so those terms are unfitted. |
| 5 | Ablation arms: `scan`, `no-base`, H-a `accum-scalar`, H-b `const-scale`, H-c `rank0`, H-d `skip-coarse` + `single-slot`, H-e `trusted` (only if gated in by 2). Arena + kinematic station, interleaved | integrating executor | 1, 2 | done (paired). `trusted` is not run (classifier). `skip-coarse`/`single-slot` are not run, because no target pose has L1/L2 rows. |
| 6 | Lever arms for hypotheses past spread (candidates: `vec3-accum` Pass B, `texel-outer` L0, `scale-shared` H-b, others as data warrants). Byte identity + oracle test per lever | integrating executor | 3, 5 | done. Levers `scan-parallel`, `scale-shared`, `texel-outer`, `vec3-accum`, `const-tile`, `array-free`(+`unroll36`/`l0-only`), `coalesced-b`. All byte-identical; the oracle passes on each. |
| 7 | Stacked arm at arena + kinematic; byte identity + oracle | integrating executor | 6 | done. Stacked = `array-free+unroll36` (arenaR, kinmidP), also measured on top of `coalesced-b`. |
| 8 | Shader statistics attempt (F7) | integrating executor | 1 | done. Unavailable; Metal GPU Counters were tried and do not discriminate the arms. |
| 9 | Findings note, 1660 handoff instructions, measurement README; move records to this branch | integrating executor | 4–8 | done. `findings.md`, README, `batches.json`. The 1660 run is outstanding (owner). |
