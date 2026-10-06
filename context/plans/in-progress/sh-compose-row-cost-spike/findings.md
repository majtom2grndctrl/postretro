# sh-compose-row-cost-spike — findings

Mac perf target: Radeon Pro 5300M on Metal, macOS 26. Records are in `measurements/sh-compose-row-cost-spike/`:
- README, `runs/`, `paired-*.json`, `model.json`;
- `batches.json`, with each batch's probe commit, binary SHA-256 and fixture prefixes;
- `capture/out/*/*/compare.json` and `capture/oracle.txt`;
- `he-gate/`.

Probe code is on the throwaway branch `sh-compose-row-cost-spike-probes`. Every batch ran one binary. Batches differ in probe commit (`1ddd56691`, `27f691833`, `c42738393`, `7a58f4833`, `6f72df62d`, `1a052cfed`), but each arm's shader rewrite is identical in every commit that carries it.

## Answer

- **The cost is fixed per row.** On every target pose, 95–99% of compose time is a fixed per-row cost, not per-entry work. The per-row costs are 3.0 µs (indirect) and 3.46 µs (Pass B), and the 97% of arena rows that carry no CSR entry pay them too.
- **The private 36-texel accumulator array (H-a) causes most of it, and dead L1/L2 code worsens it.** Ablating the array removes 43–55% of pass time. The L1/L2 path never runs on these maps, because ids 27/45 are compiled L0-only. Even so, its code costs Pass B up to 1.3 ms, through the kernel's register allocation (H-d's static cost).
- **One exact lever recovers 27–36% per pass on all four poses.** The lever is `array-free` + `unroll36` + `scale-shared`. `array-free` removes the array and the shared-lattice L1/L2 path from the kernel. `unroll36` unrolls the 36-texel loop. `scale-shared` is the per-workgroup scale cache that `array-free` requires; the probe adds it implicitly.
  - Savings: 5.1 ms of the arena's 14.7 ms compose, and 5.2 ms of the kinematic station's 17.4 ms.
  - Both passes write byte-identical atlases.
  - **Build it via a brief, labelled Metal-only until a 1660 reading arrives.**
- **The contributing-row filter is the bigger lever.** It is a separate build. The model projects it alone to take arena compose from 14.7 ms to about 1.4 ms. On top of it, this spike's lever then saves 0.17–0.84 ms per frame, depending on the pose.

## Method

- **Metric: time per compose pass encoder.** Each frame dispatches each compose pass once, as one compute encoder. `gpu_time.py` merges an encoder's nested intervals (it takes their union) and counts a coalesced row once.
- **Paired A/B within one launch.** Every reported delta comes from a paired run: `POSTRETRO_SPIKE_ARMS_B` builds a second pipeline per pass, and A and B alternate frame by frame. Both passes flip together, so a Pass B delta is always measured with the indirect pass on the same arm. The A/A null pair reads −0.01 to +0.02 ms by median (individual runs to ±0.02).
- **Spread.** Each arm reports its three paired deltas as min..max. Most fall within ±0.04 ms. Wider ranges are printed in the attribution table.
- **Retraction: the "launch regimes" were a measurement artifact.** An earlier draft said each launch fell into one of two or three GPU memory regimes, scaling the SH passes by ~1.2×. That came from the frame denominator the copied `gpu_time.py` used: the largest render-pass encoder count. Render encoders are missing from part of some traces, so that count undercounts frames by up to 26%.
  - Re-reduced per encoder, the six `diag1` launches of one binary read 6.69–6.75 / 7.99–8.02 ms. The `kinmidC` "slower regime" reads 8.00–8.04 / 9.37–9.41 ms, the same as `kinmidP`. The between-launch batch `arena3` agrees with the paired deltas (`accum-scalar` −2.98 / −4.48 vs paired −2.86 / −4.38).
  - The paired deltas never used that denominator. The ratio method once used for `coalesced-b` is unnecessary, and per encoder the conclusion is unchanged.
  - **The same heuristic is in shadow-fill-cost's `gpu_time.py`.** This brief's Basis (research.md: 3.2–4.3 / 3.6–4.9 µs per row, "25% apart across sessions") is therefore probably inflated. Per encoder, the arena reads 3.15 / 3.76 µs per row, against the 1660 Super's 0.55 / 0.77: a 5.7× / 4.9× gap, not 5–8×.
- **Validity.** 224 runs are recorded valid (foreground, unlocked, no screen saver, rows stable, arms logged). 211 of them are traced; 13 are untraced row-count probes. Discarded:
  - one invalid run, `arena2-baseline-r1`, whose screen was locked;
  - `arenaP-floor_vs_floor+no-base-r1`, which never started (a label bug). That batch was rerun as `arenaQ`.
- **Row-stability check.** It runs on whole 120-frame `[SH spike counts]` windows, so the final partial window around the trace is not checked. Rows are deterministic at a fixed pose.

## Row mix (F1)

On every pose, every composed row is L0. Ids 27/45 are uniform L0 by compiler policy (`enforce_id41_only_coarsening_policy`), so no compiler-produced map has an L1/L2 row in either pass. Today both passes compose the same rows.

| Pose | Rows | Entries ind / B | Rows with entries ind / B | Lane-entries ind / B |
|---|---|---|---|---|
| Hallway arena (`21.13,2.44,30.48`) | 2129 | 110 / 110 | 70 / 70 | 3883 / 3883 |
| Campaign-test spawn | 730 | 352 / 352 | 352 / 352 | 12201 / 12201 |
| Kinematic-platform spawn | 2579 | 780 / 75 | 780 / 75 | 43456 / 4376 |
| Kinematic station (`-6.5,1.22,-27.94`) | 2511 | 780 / 75 | 780 / 75 | 43456 / 4376 |

## Baseline and cost model (F2)

Baseline ms per frame is the median over the A halves of baseline-paired runs (p5–p95 in brackets):

| Pose | Indirect | Pass B |
|---|---|---|
| Arena | 6.71 [6.67, 6.74] | 8.00 [7.97, 8.03] |
| Campaign spawn | 2.46 [2.46, 2.53] | 2.95 [2.94, 3.03] |
| Kinematic spawn | 8.08 [8.07, 8.10] | 9.49 [9.44, 9.50] |
| Kinematic station | 8.00 [7.96, 8.04] | 9.36 [9.34, 9.39] |

The fit runs over the four poses with 4000 bootstrap draws, each picking one run per pose:

| Term | Indirect | Pass B |
|---|---|---|
| Fixed per dispatch `c` | 0.24 ms [0.21, 0.33] | 0.26 ms [0.25, 0.38] |
| Per L0 row `r` (floor fit) | 2.99 µs [2.95, 3.00] | 3.46 µs [3.41, 3.47] |
| Per L0 entry, measured per pose | 0.15–0.68 µs | 0.45–4.74 µs |
| Per L0 lane-entry, measured per pose | 4.3–19 ns | 13–90 ns |
| L1/L2 row and entry terms | **unfitted:** no L1/L2 rows exist | **unfitted** |

- **Fit quality.** Floor-fit residuals are at most 0.075 ms.
- **The entry term is not one constant, so it is reported per pose.** The floor arm's saving mixes per-entry work with per-row work that only the entry path does: the kept and rank metadata reads, and the loop setup. The share is small either way: 1.1–2.5% of indirect and 3.6–5.4% of Pass B. This departs from Decision 1's single per-entry coefficient (see plan.md Corrections).

## Top-down partition (F3, F5)

The floor arm never runs the entry loops. Pairs within the floor (`floor/floor+X`) split the floor. The ablations are not additive: `accum-scalar` also lets the compiler drop 35 of 36 dead base reads. So the remainder subtracts only the array and the stores.

| ms per frame | Arena ind | Arena B | Station ind | Station B |
|---|---|---|---|---|
| Baseline | 6.71 | 8.00 | 8.00 | 9.36 |
| **Per-entry share** (−floor Δ) | 0.07 (1.1%) | 0.35 (4.4%) | 0.20 (2.5%) | 0.36 (3.8%) |
| **Floor:** fixed per row + dispatch | 6.63 | 7.65 | 7.80 | 9.01 |
| — dispatch `c` (model) | 0.24 | 0.26 | 0.24 | 0.26 |
| — accumulator array (`floor+accum-scalar`) | 3.56 | 3.90 | 4.17 | 4.64 |
| — base / intermediate read (`floor+no-base`) | 0.27 | 1.35 | 0.23 | 1.53 |
| — stores (`floor+stores-off`) | 0.15 | 1.23 | 0.26 | 1.43 |
| — lane 0's indirection scan | unmeasured (see Attribution) | | | |
| **Floor remainder** (floor − c − array − stores) | 2.68 | 2.26 | 3.13 | 2.68 |
| **Entry-share remainder** | 0.07 (all) | 0 (over-explained) | 0.20 (all) | 0 (over-explained) |

- **Floor remainder.** It holds lane 0's scan, the indirection decodes, metadata reads, loop-bound counters and bounds clamps (H-e), the dead L1/L2 code's static cost (H-d; `single-slot` moves Pass B by 1.32 ms), and the base-read share that can't be separated from the array ablation.
- **Entry-share remainder.** On Pass B, `rank0` plus `const-scale` exceed the entry share. On indirect, every entry ablation regresses (see below), so the whole share is unattributed.

## Attribution (F4)

Paired Δ = B − A, in ms per frame: the median of 3 launches, with min..max where the range exceeds ±0.04. Types: **F** floor, **A** ablation (timing only, never recommended), **L** lever (exact), **S** stacked, **SM** stacked mix of levers that are not all recommended. The brief asks for a 1660 reading of every lever and stacked arm; all are **pending** (handoff below). Ablations need no 1660 reading.

| Hyp. | Arm | Type | Arena ind | Arena B | Station ind | Station B | 1660 ind / B |
|---|---|---|---|---|---|---|---|
| — | `floor` | F | −0.07 [−0.11, −0.07] | −0.35 | −0.20 | −0.36 [−0.39, −0.35] | pending |
| — | `floor+no-base` (vs floor) | A | −0.27 | −1.35 | −0.23 [−0.29, −0.21] | −1.53 | — |
| — | `floor+stores-off` (vs floor) | A | −0.15 | −1.23 | −0.26 | −1.43 | — |
| H-a | `accum-scalar` | A | −2.86 | −4.38 | −3.42 | −4.90 | — |
| H-a | `floor+accum-scalar` (vs floor) | A | −3.56 | −3.90 | −4.17 | −4.64 | — |
| H-a | `array-free` (+ `scale-shared`) | L | −1.55 | −2.91 | −1.50 | −2.85 | pending |
| H-a/H-e | **`array-free+unroll36`** (+ `scale-shared`) | **L/S** | **−2.23** | **−2.89** | **−2.13 [−2.21, −2.11]** | **−3.07** | pending |
| H-a/H-d | `array-free+l0-only` | L | −1.52 | −2.95 | — | — | pending |
| H-a/H-d/H-e | `array-free+unroll36+l0-only` | L | −1.24 | −3.01 | — | — | pending |
| H-a | `texel-outer` (+ `scale-shared`) | L | +2.73 | +1.07 | — | — | pending |
| H-a | `vec3-accum` (Pass B) | L | +0.01 | +0.01 | — | — | pending |
| H-b | `const-scale` | A | +0.98 | −0.16 [−0.19, −0.16] | +1.09 | −0.18 [−0.21, −0.18] | — |
| H-b | `scale-shared` | L | +1.66 | +0.01 | +1.99 [+1.93, +2.01] | +0.02 [+0.02, +0.07] | pending |
| H-c | `rank0` | A | +1.11 | −0.34 | +1.24 | −0.39 [−0.39, −0.37] | — |
| H-c | `coalesced-b` (Pass B; unpaired, per encoder, 5 launches each) | L | 6.709 → 6.717 | 8.006 → 8.007 | 8.016 → 7.999 | 9.378 → 9.378 | pending |
| H-d | `skip-coarse` | A | +0.95 [+0.92, +0.97] | −0.31 [−0.32, −0.28] | — | — | — |
| H-d | `single-slot` | A | −0.01 | −1.32 | — | — | — |
| H-e | `trusted` (wgpu checks off) | A | **not measured** (see H-e) | | | | — |
| H-e | `const-tile` (no runtime tile division) | L | +1.22 | +0.00 | — | — | pending |
| fixed | `scan-parallel` | L | +1.38 | +0.05 | — | — | pending |
| fixed | `floor+scan-parallel` (vs floor) | L | +1.49 | +0.38 | — | — | — |
| mix | `scan-parallel+array-free+const-tile+vec3-accum` | SM | −1.27 | −2.80 | — | — | — |

- **Indirect code generation is fragile.** Several arms that *remove* indirect work make indirect 0.9–2.7 ms slower: `const-scale`, `rank0`, `scale-shared`, `const-tile`, `scan-parallel`, `texel-outer`, `skip-coarse`. Pass B under the same edits moves the expected way or not at all.
  - So these arms' indirect deltas measure Metal's code generation, not the work they remove. Where an indirect ablation regresses, that hypothesis's indirect share is unattributed.
  - The pattern fits a kernel sitting at a register-allocation threshold, which is consistent with H-a and H-d's static cost.
  - The mixed stack shows the non-additivity: the four individual indirect deltas sum to +1.06 ms, but the stack measures −1.27.
- **H-a: the dominant cost.** The array ablation alone removes 3.6–4.6 ms of the floor.
- **H-b: at most 0.16–0.18 ms on Pass B; unattributable on indirect.** Its lever, `scale-shared`, regresses indirect on its own. Inside `array-free` it is required: without it, the fused texel loop would evaluate every light's scale 36 times per entry.
- **H-c: no layout lever.** Its ablation saves 0.34–0.39 ms on Pass B, about the whole entry share. The exact layout lever (texel-major repack at upload, `coalesced-b`) changes nothing per encoder. So the ablation's gain is cache reuse from every lane reading one address, not coalescing. No build.
- **H-d: no runtime share, a large static one.** The L1/L2 path never runs, but its code costs Pass B 1.32 ms (`single-slot`) and 0.31 ms (`skip-coarse`) through register allocation. `array-free` removes the shared-lattice path. Compiling out the rest (`l0-only`) adds nothing on top of it.
- **H-e:**
  - **Static gate: positive** (`he-gate/REPORT.md`). There are loop-bound counters on all 10 loops, `min` clamps on every `accum` and `delta_subblocks` access, and `naga_div`/`naga_mod` guards per texel.
  - **Timed trusted arm: not run.** The session's auto-mode permission classifier refused the `unsafe` `create_shader_module_trusted` call site, and it was not routed around. Under the brief's gate this arm should have been timed, so **H-e's share is outstanding pending the owner's ruling.**
  - **Safe partial probes.** `unroll36` drops the texel loop's counter and index math: −0.68 / −0.63 ms on indirect and +0.01 / −0.23 ms on Pass B, on top of `array-free`. `const-tile` regresses indirect and leaves Pass B unchanged.
- **Fixed term: lane 0's scan.** The exact parallel replacement, `scan-parallel` (a workgroup `atomicMin`), adds two barriers and regresses both passes. No timing-only ablation of the scan was run, so its share stays in the floor remainder. The brief's Path idea, a precomputed indirection word, needs a new binding, and Pass B's eight storage bindings are all in use.

## Shader statistics (F7)

Registers, spills and occupancy for the baseline and the H-a arms are **unavailable**. Tools tried:
- **Per-pipeline register and spill statistics:** Metal does not expose these for AMD GPUs.
- **Instruments "Metal GPU Counters" (Compute Shader Occupancy, ALU utilization, LLC bytes; 50 µs samples), recorded with Metal System Trace in paired runs** (`counters.py`, `runs/counters-arena-*.json`). Averaged over each compose label's intervals, occupancy reads ≈ 0% and ALU ≈ 100% for A and B halves alike, and LLC bytes swing by ~1000× between equivalent passes. These counters don't discriminate the arms.
- **Not tried:**
  - *Radeon GPU Analyzer* (offline, naga-emitted SPIR-V for gfx1010) needs the owner's Windows machine and shows AMD's Vulkan compiler, not Metal's. It remains the cheapest way to confirm the scratch spill.
  - *Xcode's interactive GPU frame capture* needs a GUI session.

## Stacked arm (F6)

The recommended lever is `array-free` + `unroll36`, plus the `scale-shared` that `array-free` implies. `unroll36` runs only as a modifier of `array-free`, so its "individual saving" is its increment over `array-free`, and the sum equals the stack by construction. The non-trivial additivity evidence is the mixed stack above.

| | Arena ind | Arena B | Station ind | Station B |
|---|---|---|---|---|
| Stacked, measured | −2.23 (−33%) | −2.89 (−36%) | −2.13 (−27%) | −3.07 (−33%) |
| `array-free` alone | −1.54 | −2.91 | −1.50 | −2.85 |
| `unroll36` increment | −0.68 | +0.01 | −0.63 | −0.23 |
| Stacked on the `coalesced-b` layout (paired vs `coalesced-b`) | −2.20 | −2.82 | −2.18 | −3.23 |

Campaign spawn: −0.77 / −0.98 ms (−31% / −33%). Kinematic spawn: −2.19 / −3.05 ms.

## Projection through the contributing-row filter

- **Formula.** The filter keeps only rows with entries (`rows_f`). `T_post = c + r·rows_f + entry share`.
- **Lever on top.** Its post-filter saving is its per-row saving (stacked Δ / rows) × `rows_f`.
  - The per-row saving matches across poses with very different entry fractions: 1.05 µs/row (indirect) and 1.36 µs/row (Pass B) at the arena, where 3% of rows carry entries, and at campaign spawn, where 48% do. So rows with entries don't save differently.
  - `T_post` extrapolates `c + r·rows` down to 70–780 rows; the model was fitted on 730–2579.

| Pose | Today ind / B (ms) | Filter alone ind / B | Lever on top ind / B | Lever share of post-filter compose |
|---|---|---|---|---|
| Arena | 6.71 / 8.00 | 0.52 / 0.86 | −0.07 / −0.10 | 12% |
| Campaign spawn | 2.46 / 2.95 | 1.34 / 1.64 | −0.37 / −0.47 | 28% |
| Kinematic spawn | 8.08 / 9.49 | 2.78 / 0.87 | −0.66 / −0.09 | 21% |
| Kinematic station | 8.00 / 9.36 | 2.77 / 0.88 | −0.66 / −0.09 | 21% |

## Byte identity (H4, H5)

**What was compared.** Capture dumps read back the composed `rgba16float` indirect and direct atlases: whole arrays, 2560×2552×4 each, read after the final frame. They are compared with the baseline dumps of probe commit `1ddd56691`.
- **Scenes.** Three views (stress-warren-mini animroom, the hallway arena, kinematic station) × gated and force-full-resident × t = 0.5 s and 1.0 s, so 12 scenes.
- **Distinct states.** They hold 9 distinct indirect states and 8 distinct direct states: at the arena and kinematic station some time or mode pairs coincide. Animroom and kinematic station still give two stepped times, streamed and full-resident.
- **Determinism.** The baseline repeats byte-identically across four probe binaries (`@repeat`, `@b2`, `@b3`, `@b4`).

**Results.** Records: `capture/out/<arm>/<scene>/compare.json`; oracle runs: `capture/oracle.txt`.
- **Every lever and every stack is byte-identical in all 12 scenes.** That covers `scan-parallel`, `scale-shared`, `texel-outer`, `array-free` (alone, `+unroll36`, `+l0-only`, both), `vec3-accum`, `const-tile`, `coalesced-b`, and the stacks.
- **Negative control: `floor` differs in all 12 scenes, in both atlases.** For example, at arena-gated t = 0.5 s, 24,523 indirect texels differ (max RGB deviation 0.083) and 51 direct texels differ.
- **Oracle test.** `sampled_row_gate_capture_matches_full_resident_at_stepped_times` passes with each lever's and each stack's arms set: 14 runs.

**Coverage gaps** (the follow-on must close or accept them):
- **L1/L2 path.** No fixture has an L1/L2 row in ids 27/45. `array-free`'s L1/L2 reconstruction matches the original in source (same slot order and arithmetic, traced by review), but its bytes are unproven. Metal fast-math may contract it differently. Proving it needs a GPU atlas byte-compare on Metal; a CPU unit test cannot see code generation.
- **Scale cache fallback.** No fixture row has more than 64 entries, so the fallback is never exercised.
- **Legacy pipelines.** The non-streamed pipelines (`sh_compose.rs`, `animated_direct_sh_compose.rs`) share the rewrites but were never compared.
- **Levels ≥ 3.** `array-free` would reconstruct differently there. The loader rejects them today; the build should gate reconstruction on `level == 1u`.

## Recommendations (build calls)

| Lever | Label | Byte identity | Call |
|---|---|---|---|
| `array-free` + `unroll36` + `scale-shared`, both passes | **Metal-only** (no 1660 reading) | identical, 12/12 + oracle, L0 content. The L1/L2 path is unproven by bytes. | **Build, via a brief** |
| `l0-only` | — | identical (L0 content only) | No build: no gain over `array-free` |
| `texel-outer`, `scale-shared` alone, `scan-parallel`, `const-tile` | — | identical | No build: they regress indirect |
| `vec3-accum` | — | identical | No build: no effect |
| `coalesced-b` | — | identical | No build: no gain |

**Follow-on brief: `sh-compose-array-free`.** A brief rather than a direct build, because it has these decisions to make:
1. **Indirect code-generation fragility.** Small edits move indirect by ±1–2.7 ms. The build should land the measured shape, and its acceptance should include a Mac paired A/B re-measure.
2. **The L1/L2 path in 27/45.** Either keep `array-free`'s L1/L2 reconstruction and prove it with a Metal GPU byte-compare on a synthetic L1/L2 fixture, or have the loader reject non-L0 levels in 27/45 and delete the path. The second is a format-contract decision.
3. **The 1660.** A Metal-only lever still ships to the 1660 perf floor (§8 has no per-backend variants), so landing it needs a 1660 no-regression reading.
4. **Sequencing.** Run it after the contributing-row filter. The filter is the 10× lever, and the rows it leaves are the ones this lever's per-row saving applies to.

## Open for the owner

- **H-e timed arm.** Either allow the `unsafe` trusted-module call site on the probes branch, or rule H-e reported from the static gate and the safe partial probes.
- **Cost-model form.** The entry term is reported per pose rather than as one coefficient, and the lever projection scales the per-row saving. See plan.md Corrections.
- **The brief's Basis numbers are probably inflated** by the shared `gpu_time.py` frame heuristic (see Method). The same heuristic sits in `measurements/shadow-fill-cost/`.

## 1660 Super handoff (F8): pending

On the owner's Windows machine, check out the probes branch at `1a052cfed`. Keep that commit until the handoff is done. Then run:

```
cargo run -p xtask -- run --release -- content/dev/maps/<map>.prl [--start-pose=…]
```

- **Arm per launch.** Set `POSTRETRO_SPIKE_ARMS=<arm>` per launch, unpaired. GPU timestamps work there and separate the arms; paired mode would average A and B together.
- **GPU timing.** Set `POSTRETRO_GPU_TIMING=1 RUST_LOG=info`.
- **Priority arms:** `baseline`, `array-free`, `array-free,unroll36`. Optional: `floor` and the no-build levers.
- **Poses:** campaign-test spawn, and the hallway arena (`--start-pose=21.13,2.44,30.48,0,0`).
- **Protocol:** three launches each, interleaved. Read `sh_compose` and `animated_direct_sh_compose` from the `[GpuTiming]` lines.
- **What it decides:** a regression in either pass makes the lever not recommended; a win relabels it all-backends.
