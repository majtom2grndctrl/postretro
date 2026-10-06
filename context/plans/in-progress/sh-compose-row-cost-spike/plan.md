# sh-compose-row-cost-spike — plan of record

mode: compact
status: active
read at: 42cec06d8

## Layout

- Worktree `../postretro-shspike`, branch `sh-compose-row-cost-spike`. It holds this plan, `measurements/sh-compose-row-cost-spike/`, and the findings note. It is the only branch that lands.
- Worktree `../postretro-shspike-probes`, branch `sh-compose-row-cost-spike-probes`, cut from this branch. It holds every shader, pipeline and instrumentation probe, including the `unsafe` H-e call site. It is deleted unmerged when the spike lands.
- Builds use `CARGO_TARGET_DIR=/Users/dhiester/Projects/Personal/postretro/target`, so dependencies are shared with the main tree. Disk had 18 GB free at claim time, and a second release target would cost about 8 GB.
- The PRL fixtures are gitignored, so each worktree symlinks `content/dev/maps/*.prl` from the main tree.

## Corrections

- Brief says wgpu 29.0.1 / wgpu-hal 29 (H-e). Since `de735f026` the stack is **wgpu 30.0.1** (`1ad33de46`). `wgpu-hal-30.0.1/src/metal/device.rs` still picks `BoundsCheckPolicy::Restrict` unless runtime checks are off, and passes `force_loop_bounding` through. `Device::create_shader_module_trusted` is still the only way in, and it is still `unsafe`. The premise holds; only the version changes.
- Path: "`ShProbeReadback::encode_copy` already copies the total atlas." It copies only the indirect total atlas. It is also an async dev-tools overlay readback. The byte comparison needs Pass B's `direct_composed_atlas` as well. So the probe branch adds a synchronous capture-time dump of both composed `rgba16float` atlases instead.
- Fixture `campaign-test.prl` was re-baked on 2026-10-05. Its prefix is now `2470294d8cb54730`, not shadow-fill-cost's `574be03376fdb9cf`. The other two fixtures match shadow-fill-cost: hallway-inspection `a543d0e0291dbceb`, kinematic-platform `8939e012bc683c0c`. The compose shaders and their pipelines have not changed since `de735f026`.
- Arm selection: one release binary carries every arm. An environment variable (`POSTRETRO_SPIKE_ARMS`, a comma list, so arms stack) is read at pipeline creation and rewrites the WGSL source. The brief allows this ("by an environment variable read at pipeline creation"). All arms then share one build lineage, which removes a binary-layout confound between arms. The `unsafe` trusted module sits behind the off-by-default cargo feature `spike-trusted-compose`. The env arm `trusted` is honoured only when that feature is compiled in.
- Each arm rewrites both compose shaders where the hypothesis applies to both. Metal System Trace labels the two passes separately, so one run yields both passes' deltas.

## Delegated answers

- Rounding-only lever (FMA contraction) — the note reports the lever's maximum absolute deviation over the dumped atlases and marks the recommendation conditional, per the brief.
- Pass B L1/L2 entry counts too small to fit — the note first adds poses whose per-level counts differ. Any term still unidentifiable is reported unfitted and named. The floor and skip-coarse arms then give a single-pose share as a cross-check.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| H1 one commit + fixture set per batch, SHA-256 prefixes | batch manifest (`measurements/.../batches/*.json`: probe commit, binary hash, fixture prefixes) | achievable as stated |
| H2 foreground/unlocked/no-saver per run, idle ioreg per batch, invalid count | `run.py` validity record + `*-idle.ioreg.txt`; summary counts discarded runs | achievable as stated |
| H3 ≥3 traces per arm per pose with spread; discard runs whose `compose frame` rows change | `summarize.py` over `*-gpu.json` + per-run `[SH streaming]` row check | achievable as stated |
| H4 lever + stacked arms byte-identical to baseline (≥2 stepped times, streamed + full-resident), oracle test passes | atlas-dump compare script; `cargo test -p postretro --features capture --test capture_frame sampled_row_gate_capture_matches_full_resident_at_stepped_times -- --ignored` with the arm env set | achievable as stated |
| H5 negative control differs | same compare script on one ablation arm | achievable as stated |
| H6 no ablation/floor arm recommended; every arm typed | findings note arm table | achievable as stated |
| F1 rows and entries by brick level per pose and pass | probe-branch `[SH spike counts]` log line, recorded per run | achievable as stated |
| F2 baseline per-pass ms at arena, kinematic spawn + station, campaign spawn; fitted model with spread | traces + `fit_model.py` (bootstrap over runs) | achievable as stated |
| F3 floor share per pass | floor arm traces | achievable as stated |
| F4 H-a..H-e ablation deltas (+ lever where built); H-e unmeasured if MSL gate is clean | ablation traces; MSL gate record | achievable as stated |
| F5 remainder | arithmetic over F3/F4 in the note | achievable as stated |
| F6 stacked saving vs sum of levers, arena + kinematic | stacked traces | achievable as stated |
| F7 shader statistics or "unavailable" with tools tried | Xcode GPU capture / `metal` toolchain attempt record | achievable as stated |
| F8 1660 Super per-pass ms | owner handoff | manual (owner, Windows) — pending unless returned |
| N1 findings note | `findings.md` in this folder | achievable as stated |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Probe infrastructure on the probes branch: arm rewrite (`baseline`, `floor`) for both passes, streamed and legacy pipelines; per-level row/entry count log; capture atlas dump. Copy measurement tooling. Then the riskiest premise: baseline vs floor at the arena, 3 interleaved traces each. Does the per-pass spread resolve a split, and do the rows stay stable? | integrating executor | — | |
| 2 | H-e static gate: emit both composed shaders' MSL with checks on and off (naga safe API), count checks inside the per-entry/per-texel loops | delegated (read-only + scratch test) | — | |
| 3 | Atlas byte-compare harness (dump → compare, max abs deviation); negative control on `floor` | integrating executor | 1 | |
| 4 | Baseline + floor at the 4 brief poses plus extra poses for the per-level fit; F1 counts; model fit | integrating executor | 1 | |
| 5 | Ablation arms: `scan`, `no-base`, H-a `accum-scalar`, H-b `const-scale`, H-c `rank0`, H-d `skip-coarse` + `single-slot`, H-e `trusted` (only if gated in by 2). Arena + kinematic station, interleaved | integrating executor | 1, 2 | |
| 6 | Lever arms for hypotheses past spread (candidates: `vec3-accum` Pass B, `texel-outer` L0, `scale-shared` H-b, others as data warrants). Byte identity + oracle test per lever | integrating executor | 3, 5 | |
| 7 | Stacked arm at arena + kinematic; byte identity + oracle | integrating executor | 6 | |
| 8 | Shader statistics attempt (F7) | integrating executor | 1 | |
| 9 | Findings note, 1660 handoff instructions, measurement README; move records to this branch | integrating executor | 4–8 | |
