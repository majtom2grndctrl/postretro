# sh-compose-row-cost-spike — plan of record

mode: compact
status: landed
read at: 42cec06d8

(The brief was read at `de735f026`. This plan re-verified its cited source at the claim commit `42cec06d8`; see Corrections.)

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
- **In-launch pairs (H3 method).** With `POSTRETRO_SPIKE_ARMS_B`, each compose pass builds A and B pipelines that alternate per frame, and B passes are labelled `… [B]`. Every reported delta comes from these pairs. The A/A null pair reads within ±0.02 ms. This keeps the brief's interleaving rule, per frame inside one launch.
  - **Retracted:** "launch memory regimes". The motivation first recorded here was that launches fell into regimes scaling the SH passes ~1.2×. Review found that this was the copied `gpu_time.py`'s frame denominator: the largest render-pass encoder count, which undercounts when render encoders are missing from part of a trace.
  - **Also fixed in review:** nested encoder rows were summed, and a coalesced row was multiplied.
  - **The fix:** `gpu_time.py` now takes the union of each encoder's intervals and counts frames from the compose encoders. All 202 traces were re-reduced from `raw/`.
  - **What changes:** per encoder, there is no regime (`diag1` 6.69–6.75 ms). The `coalesced-b` ratio method was unnecessary, and its per-encoder comparison gives the same answer. The paired deltas never used the denominator, but the nested-row fix moved a few cells by ≤ 0.05 ms.
  - **Same heuristic upstream:** shadow-fill-cost's `gpu_time.py`, so the brief's Basis µs/row figures are probably inflated (findings §Method).
- **Added levers beyond the brief's candidates:** `array-free` (+ `unroll36`, `l0-only`) and `coalesced-b`. The brief's rule is that *only* a hypothesis whose ablation moves beyond spread gets a lever.
  - `array-free` follows from H-a's ablation, and `coalesced-b` from H-c's ablation on Pass B.
  - `scan-parallel` and `const-tile` were built without a qualifying ablation (the scan's share is unmeasured, and H-e's trusted ablation was not run). Neither is recommended.
- **The recommended lever includes `scale-shared`.** `array-free` (and `texel-outer`) implies the per-workgroup scale cache: the probe adds it, and every run log and capture log shows `…,scale-shared`. So the measured, byte-tested lever is `array-free` + `unroll36` + `scale-shared`. Review caught the first findings draft naming only two of the three.
- **The cost model departs from Decision 1's form. Owner to acknowledge.**
  - Decision 1 asks for per-entry terms by level. The fit gives `c` and `r` with spread. The per-entry term varies across poses (0.15–0.68 µs indirect, 0.45–4.74 µs Pass B), because the floor saving also removes per-row work of the entry path. So it is reported per pose, not as one coefficient.
  - The L1/L2 terms are unfitted (no rows).
  - The lever's post-filter projection scales its measured per-row saving by `rows_f`, not by model terms. The per-row saving matches at 3% and 48% entry-row fractions.
- **H-d ablations were run late** (batch `arenaD`, after review). On L0-only content they measure the L1/L2 path's static cost: `single-slot` −1.32 ms on Pass B.
- **Shared target dir, shared binary path.** The probe release binary is written to the main tree's `target/release/postretro`. Each probe build is copied at once to `scratchpad/bin/probe-<commit>/`, and batches run that copy.

## Delegated answers

- Rounding-only lever (FMA contraction) — the note reports the lever's maximum absolute deviation over the dumped atlases and marks the recommendation conditional, per the brief.
- Pass B L1/L2 entry counts too small to fit — the note first adds poses whose per-level counts differ. Any term still unidentifiable is reported unfitted and named. The floor and skip-coarse arms then give a single-pose share as a cross-check.

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| H1 one commit + fixture set per batch, SHA-256 prefixes | `measurements/sh-compose-row-cost-spike/batches.json` (`manifest.py`): per batch, the probe commit and binary SHA-256 from every run record, plus fixture prefixes | achievable as stated | **pass**. Every batch ran one binary. Fixtures `a543d0e0291dbceb` / `2470294d8cb54730` / `8939e012bc683c0c`. No probe touches row planning, so no batch straddles a change to which rows compose. |
| H2 foreground/unlocked/no-saver per run, idle ioreg per batch, invalid count | `run.py` validity record, `runs/*-idle.ioreg.txt` | achievable as stated | **pass**. 224 valid runs (211 traced). 1 invalid (`arena2-baseline-r1`, screen locked; its batch stopped). 1 run never started (`arenaP`, label bug; batch rerun as `arenaQ`). The untraced `counts`/`mix`/`counters` probes ran outside `batch.sh`, with no idle snapshot. |
| H3 ≥3 traces per arm per pose with spread; discard runs whose rows change | `summarize_paired.py` → `paired-*.json`; `rows_stable` in each run record | achievable as stated | **pass**. Every reported arm has 3 paired launches (5 unpaired launches for `coalesced-b`). Ranges wider than ±0.04 ms are printed in findings. No valid run changed its rows. The check covers whole 120-frame windows only. The "launch regime" claim is retracted (see Corrections). |
| H4 lever + stacked byte-identical to baseline (≥2 stepped times, streamed + full-resident), oracle passes | `capture.sh` + `compare_atlas.py` (12 scenes); oracle test with the arm env | achievable as stated | **pass, on L0 content**. Every lever and stack is byte-identical in 12/12 scenes (`capture/out/*/*/compare.json`; 9/8 distinct states). The oracle passes in 14 runs (`capture/oracle.txt`): 11 lever or stack arm sets, including the recommended one, plus `baseline` ×2 and `floor`. `array-free+l0-only` and the mixed stack were byte-compared but not oracle-run. Gap: there is no L1/L2 fixture for 27/45 (the compiler never emits one), so `array-free`'s L1/L2 path is unproven by bytes. |
| H5 negative control differs | `floor` through the same compare | achievable as stated | **pass**. `floor` differs in 12/12 scenes, in both atlases. |
| H6 no ablation/floor arm recommended; every arm typed | findings §Attribution and §Recommendations | achievable as stated | **pass**. Only `array-free+unroll36` (+ the implied `scale-shared`) is recommended. |
| F1 rows and entries by brick level per pose and pass | `[SH spike counts]`, `runs/counts-*` | achievable as stated | **reported**. All rows are L0 on every pose, with 0 L1/L2. Rows with entries and lane-entries are added. |
| F2 baseline per-pass ms at 4 poses; fitted model with spread | `model.json` (`fit_model.py`, bootstrap) | achievable as stated | **reported, with a model-form departure** (Corrections). `c` = 0.24 / 0.26 ms; `r` = 2.99 / 3.46 µs per row. The per-entry term is reported per pose, and the L1/L2 terms are unfitted. |
| F3 floor share per pass | paired `floor` | achievable as stated | **reported**. The entry share is 1.1–2.5% (indirect) and 3.6–5.4% (Pass B). |
| F4 H-a..H-e ablation deltas (+ lever where built) | findings §Attribution | achievable as stated | **reported; H-e accepted from the static gate and safe probes (owner, 2026-10-06).** Originally outstanding: The static gate is positive, so the brief requires the timed `trusted` arm. It was **not run**, because the permission classifier blocked the `unsafe` call site. Owner ruling needed: allow it, or accept the static gate plus the safe partial probes (`unroll36`, `const-tile`). H-d ablations ran (`arenaD`). |
| F5 remainder | findings §Top-down partition | achievable as stated | **reported** |
| F6 stacked saving vs sum of levers, arena + kinematic | paired `array-free+unroll36` | achievable as stated | **reported**. −2.23 / −2.89 ms (arena), −2.13 / −3.07 ms (station). The sum is tautological for the recommended stack. The mixed stack `scan-parallel+array-free+const-tile+vec3-accum` shows the non-additivity. |
| F7 shader statistics or "unavailable" with tools tried | findings §Shader statistics | achievable as stated | **unavailable**. Tools tried are named there. |
| F8 1660 Super per-pass ms | owner handoff | manual | **pending (owner)**. It gates promotion of `sh-compose-array-free`; the probes branch is pushed for it. Pending for every lever and stacked arm. The handoff prioritizes baseline, `array-free` and `array-free,unroll36`. |
| N1 findings note | `findings.md` | achievable as stated | **pass** |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Probe infrastructure on the probes branch: arm rewrite (`baseline`, `floor`) for both passes, streamed and legacy pipelines; per-level row/entry count log; capture atlas dump. Copy measurement tooling. Then the riskiest premise: baseline vs floor at the arena, 3 interleaved traces each. Does the per-pass spread resolve a split, and do the rows stay stable? | integrating executor | — | done. Probes `27c10a573`, `1ddd56691`; `every_arm_rewrites_and_validates` passes. Batch `arena1` (6 of 6 valid, rows stable at 2129 L0 rows / 110 L0 entries per pass): per-encoder re-reduction: floor 6.62 / 7.64 ms against baseline ~6.7 / ~8.0. The split resolves, and the fixed per-row term is ~95–98% at this pose. These were the first, per-frame numbers; superseded by the paired batches. |
| 2 | H-e static gate: emit both composed shaders' MSL with checks on and off (naga safe API), count checks inside the per-entry/per-texel loops | delegated (read-only + scratch test) | — | done. Checks sit inside the per-entry and per-texel loops in both shaders (`measurements/sh-compose-row-cost-spike/he-gate/REPORT.md`). |
| 3 | Atlas byte-compare harness (dump → compare, max abs deviation); negative control on `floor` | integrating executor | 1 | done. 12 scenes × (baseline, every arm); dumps of 3.6 GB per arm are compared and then deleted. |
| 4 | Baseline + floor at the 4 brief poses plus extra poses for the per-level fit; F1 counts; model fit | integrating executor | 1 | done. Batches `arenaQ`, `campaignP`, `kinspawnP`, `kinmidP`; `model.json`. No pose has L1/L2 rows, so those terms are unfitted. |
| 5 | Ablation arms: lane-0 scan (built as the lever `scan-parallel`), `no-base`, H-a `accum-scalar`, H-b `const-scale`, H-c `rank0`, H-d `skip-coarse` + `single-slot`, H-e `trusted` (only if gated in by 2). Arena + kinematic station, interleaved | integrating executor | 1, 2 | done (paired). `trusted` is not run (classifier; owner ruling pending). `skip-coarse`/`single-slot` ran at the arena after review (`arenaD`). |
| 6 | Lever arms for hypotheses past spread (candidates: `vec3-accum` Pass B, `texel-outer` L0, `scale-shared` H-b, others as data warrants). Byte identity + oracle test per lever | integrating executor | 3, 5 | done. Levers `scan-parallel`, `scale-shared`, `texel-outer`, `vec3-accum`, `const-tile`, `array-free`(+`unroll36`/`l0-only`), `coalesced-b`. All byte-identical (`compare.json`); oracle runs are in `capture/oracle.txt` (every lever except `l0-only`). |
| 7 | Stacked arm at arena + kinematic; byte identity + oracle | integrating executor | 6 | done. Stacked = `array-free+unroll36` + implied `scale-shared` (arenaR, kinmidP), also on top of `coalesced-b`. Oracle passes on it. |
| 8 | Shader statistics attempt (F7) | integrating executor | 1 | done. Unavailable; Metal GPU Counters were tried and do not discriminate the arms. |
| 9 | Findings note, 1660 handoff instructions, measurement README; move records to this branch | integrating executor | 4–8 | done. `findings.md`, README, `batches.json`. The 1660 run is outstanding (owner). |
