# sh-compose-row-cost-spike — findings

Mac perf target: Radeon Pro 5300M on Metal, macOS 26. Records: `measurements/sh-compose-row-cost-spike/` (README, `runs/`, `paired-*.json`, `model.json`, `he-gate/`). Probe code: the throwaway branch `sh-compose-row-cost-spike-probes`. Batches ran from probe commits `c42738393`, `7a58f4833` and `1a052cfed`. Each batch used one commit, and the arms' shader rewrites are identical across those commits.

## Answer

- **The cost is fixed per row.** On every target pose, 95–99% of compose time is fixed per row, not per entry. The 3.0 µs (indirect) and 3.45 µs (Pass B) per row are paid even by the 97% of arena rows that carry no CSR entry.
- **The private 36-texel accumulator array (H-a) causes most of it.** Ablating the array removes 43–55% of pass time.
- **One exact lever recovers 27–36% per pass on all four poses: `array-free` + `unroll36`.** `array-free` removes the array from the kernel. `unroll36` unrolls the 36-texel loop. Together they save 5.1 ms of the arena's 14.7 ms and 5.2 ms of the kinematic station's 17.3 ms. Both passes write byte-identical atlases. **Build it via a brief, Metal-only until a 1660 reading arrives.**
- **The separate contributing-row filter is the bigger lever.** The model projects it alone to take arena compose from 14.7 ms to about 1.4 ms. After it lands, this spike's lever saves 0.17–0.85 ms per frame, depending on the pose.

## Method notes (read before the numbers)

- **Launch regimes.** Each engine launch lands in one of two or three GPU memory regimes. They scale every SH-atlas pass by up to ~1.2× together: both compose passes, plus Billboard Direct Scatter Compose. Core clock (1232 MHz), activity (99%), temperature and memory clock do not predict the regime (`runs/diag1-*`). Between-launch comparison was therefore unreadable (`arena3`: baseline 6.71–8.19 ms).
- **Paired A/B, the method every delta uses.** Two pipelines are built per pass and alternate frame by frame inside one launch. B frames carry the label `… [B]`, and the metric is time per pass encoder. The A/A null pair reads −0.006 to +0.016 ms by median (individual runs to ±0.08 ms). Every arm's three paired deltas fall within about ±0.05 ms of each other.
- **Ratio method for `coalesced-b`.** A layout change cannot pair inside one launch, because both halves share the buffers. That arm changes only Pass B, so it is compared by Pass B ÷ indirect, which stays between 1.19 and 1.20 across regimes.
- **Validity.** 215 runs are recorded valid: foreground, unlocked, no screen saver, stable composed rows (`[SH spike counts]` min = max after the first window), arm logged. 202 of them are traced; 13 are untraced row-count probes. Discarded:
  - one invalid run, `arena2-baseline-r1`, whose screen was locked. Its batch stopped there.
  - `arenaP-floor_vs_floor+no-base-r1`, which never started: a label bug turned the name into a path. That batch was rerun whole as `arenaQ`.

  Batch `arena3` (42 valid runs) is kept but not used for deltas, because of the regime confound.

  Batch `arena1` ran before `baked/` was linked, so material textures were placeholders. It is preliminary, and its numbers here are corroborated by later paired batches.

## Row mix (F1)

On every pose, every composed row is L0. Ids 27/45 are uniform L0 by compiler policy (`enforce_id41_only_coarsening_policy`), so no compiler-produced map has L1/L2 rows in either pass. Rows are the same for both passes today.

| Pose | Rows (both passes) | Entries ind / B | Rows with entries ind / B | Lane-entries ind / B |
|---|---|---|---|---|
| Hallway arena (`21.13,2.44,30.48`) | 2129 | 110 / 110 | 70 / 70 | 3883 / 3883 |
| Campaign-test spawn | 730 | 352 / 352 | 352 / 352 | 12201 / 12201 |
| Kinematic-platform spawn | 2579 | 780 / 75 | 780 / 75 | 43456 / 4376 |
| Kinematic station (`-6.5,1.22,-27.94`) | 2511 | 780 / 75 | 780 / 75 | 43456 / 4376 |

## Baseline and cost model (F2)

Baseline per-pass ms per frame is the median over all paired runs' A halves. These came from the fast regime. The station also read 9.6–9.9 / 11.2–11.6 ms in a slower regime (`kinmidC`).

| Pose | Indirect ms | Pass B ms |
|---|---|---|
| Arena | 6.70 | 7.99 |
| Campaign spawn | 2.46 | 2.95 |
| Kinematic spawn | 8.08 | 9.49 |
| Kinematic station | 7.98 | 9.36 |

The model is fitted over the four poses, with 4000 bootstrap draws picking one run per pose (p5–p95 in brackets):

| Term | Indirect | Pass B |
|---|---|---|
| Fixed per dispatch `c` | 0.24 ms [0.21, 0.36] | 0.27 ms [0.22, 0.44] |
| Per L0 row `r` (floor fit) | 2.98 µs [2.91, 3.00] | 3.45 µs [3.33, 3.47] |
| Per L0 entry | 0.15–0.68 µs; not one constant across poses | 0.45–5.2 µs; not one constant |
| Per L0 lane-entry | 4.3–19 ns | 13–90 ns |
| L1/L2 row and entry terms | **unfitted:** no L1/L2 rows exist (compiler policy) | **unfitted:** same |

Floor-fit residuals are at most 0.064 ms. The entry term does not fit one constant because the floor arm's saving mixes per-entry work with per-row work that only the entry path does: the `local_probe_is_kept`/`within_cell_rank` metadata reads and the loop setup. The entry share is small either way: 1.1–2.5% of indirect and 3.6–5.4% of Pass B.

## Top-down partition (F3, F5)

Floor arm: entry loops never run. Pairs within the floor (`floor/floor+X`) split the floor. The ablations interact: `accum-scalar` also lets the compiler drop 35 of 36 dead base reads. So shares are not summed across the base read, and the remainder below subtracts only the array and the stores.

| ms per frame | Arena ind | Arena B | Station ind | Station B |
|---|---|---|---|---|
| Baseline | 6.70 | 7.99 | 7.98 | 9.36 |
| **Per-entry share** (−floor Δ) | 0.07 (1.1%) | 0.35 (4.4%) | 0.20 (2.5%) | 0.39 (4.2%) |
| **Fixed per-row + dispatch** (floor) | 6.62 | 7.64 | 7.78 | 8.97 |
| — dispatch `c` | 0.24 | 0.27 | 0.24 | 0.27 |
| — accumulator array (`floor+accum-scalar`) | 3.56 | 3.86 | 4.17 | 4.64 |
| — base / intermediate read (`floor+no-base`) | 0.27 | 1.35 | 0.23 | 1.58 |
| — stores (`floor+stores-off`) | 0.15 | 1.25 | 0.26 | 1.43 |
| — lane 0's indirection scan | unmeasured (see H-fixed) | | | |
| **Floor remainder** (floor − c − array − stores) | 2.67 | 2.26 | 3.11 | 2.63 |
| **Entry-share remainder** | 0.07 (all) | 0 (over-explained) | 0.20 (all) | 0 (over-explained) |

- **Floor remainder.** It holds lane 0's scan, indirection decodes, the metadata reads, loop-bound counters and bounds clamps (H-e), and the base-read share not separable from the array ablation. No arm isolates it.
- **Entry-share remainder.** On Pass B, `rank0` plus `const-scale` (0.50 / 0.58 ms) exceed the entry share, because the ablations interact. On indirect, no ablation moved the entry share downward, so all of it is unexplained (see the code-generation note under the attribution table).

## Attribution (F4)

Paired Δ = B − A in ms per frame (median of 3 launches, ± ≤ 0.05 unless noted). Types: **F** floor, **A** ablation (timing only), **L** lever (exact), **S** stacked. All 1660 cells are pending the owner handoff (below).

| Hyp. | Arm | Type | Arena ind | Arena B | Station ind | Station B | 1660 |
|---|---|---|---|---|---|---|---|
| — | `floor` | F | −0.07 | −0.35 | −0.20 | −0.39 | pending |
| H-a | `accum-scalar` | A | −2.86 | −4.38 | −3.42 | −4.90 | — |
| H-a | `floor+accum-scalar` (vs floor) | A | −3.56 | −3.86 | −4.17 | −4.64 | — |
| H-a | `array-free` | L | −1.55 | −2.91 | −1.50 | −2.85 | pending |
| H-a/H-e | `array-free+unroll36` | L/S | **−2.23** | **−2.89** | **−2.13** | **−3.04** | pending |
| H-a/H-d | `array-free+l0-only` | L | −1.52 | −2.95 | — | — | — |
| H-a/H-d/H-e | `array-free+unroll36+l0-only` | L | −1.24 | −3.01 | — | — | — |
| H-a | `texel-outer` (L0 only, implies `scale-shared`) | L | +2.73 | +1.07 | — | — | — |
| H-a | `vec3-accum` (Pass B) | L | +0.01 | +0.01 | — | — | — |
| H-b | `const-scale` | A | +0.93 | −0.16 | +1.09 | −0.21 | — |
| H-b | `scale-shared` | L | +1.66 | +0.01 | +1.99 | +0.02 | — |
| H-c | `rank0` | A | +1.11 | −0.34 | +1.24 | −0.37 | — |
| H-c | `coalesced-b` (Pass B, ratio method) | L | n/a | ratio 1.1917 → 1.1908 (spread ±0.005) | n/a | 1.163 → 1.172 (overlapping) | — |
| H-d | `skip-coarse`, `single-slot` | A | not run: no L1/L2 rows on any target pose, so H-d's runtime share is 0 | | | | — |
| H-e | `trusted` (wgpu checks off) | A | **not measured** (see H-e) | | | | — |
| H-e | `const-tile` (no runtime tile division) | L | +1.23 | +0.00 | — | — | — |
| fixed | `scan-parallel` | L | +1.38 | +0.05 | — | — | — |
| fixed | `floor+scan-parallel` (vs floor) | L | +1.50 | +0.37 | — | — | — |

- **Indirect code generation is fragile.** Several arms that *remove* indirect work make indirect 0.9–2.7 ms slower: `const-scale`, `rank0`, `scale-shared`, `const-tile`, `scan-parallel`, `texel-outer`. Pass B under the same edits moves the expected way or not at all. So their indirect deltas measure Metal's code generation, not the work removed. An indirect hypothesis share is therefore unattributed whenever its ablation regresses. The pattern fits a kernel near a register-allocation threshold, which is consistent with H-a and with H-e's loop bounding. `unroll36` (which drops the texel loop's counter) gains 0.68 ms on indirect, and adding `l0-only` gives 1 ms back.
- **H-b** is at most 0.16–0.21 ms on Pass B and unattributable on indirect. Its lever regresses indirect. No build.
- **H-c.** Its ablation saves 0.34–0.37 ms on Pass B, about its whole entry share. The exact layout lever (texel-major repack at upload, `coalesced-b`) does not move Pass B beyond spread, so the ablation's gain is cache reuse from all lanes reading one address, not coalescing. No build.
- **H-d** has a runtime share of 0 on all content: ids 27/45 are never coarsened. Its *static* cost is real, though. The L1/L2 path's shared lattice and per-entry accumulation are why the kernel holds the 36-entry array. Compiling the path out (`l0-only`) adds nothing on top of `array-free`.
- **H-e:**
  - *Static gate:* checks do sit inside the hot loops (`he-gate/REPORT.md`): loop-bound counters on all 10 loops, `min` clamps on every `accum` and `delta_subblocks` access, and `naga_div`/`naga_mod` guards per texel.
  - *Timed trusted arm:* not run. The session's auto-mode permission classifier refused the `unsafe` `create_shader_module_trusted` call site, and it was not routed around.
  - *Safe partial probes:* `unroll36` removes the texel loop's counter and index math, gaining −0.68 ms on indirect and 0 on Pass B on top of `array-free`. `const-tile` removes the runtime tile divisions and regresses indirect, with no change on Pass B.
  - *Reporting:* H-e is reported partially measured. Its full share stays inside the floor remainder.
- **Fixed term (lane 0's scan).** The exact parallel replacement (`scan-parallel`, a workgroup `atomicMin`) adds two barriers and regresses both passes. No pure removal of the scan preserves which rows store, so its share stays in the floor remainder.

## Shader statistics (F7)

Registers, spills and occupancy for the baseline and the H-a arms are **unavailable**. Tools tried:
- **Per-pipeline register and spill statistics:** Metal does not expose these for AMD GPUs.
- **Instruments "Metal GPU Counters" (Compute Shader Occupancy, ALU utilization, LLC bytes; 50 µs samples), recorded with Metal System Trace in paired runs** (`counters.py`, `runs/counters-arena-*.json`):
  - averaged over each compose pass label's intervals, occupancy reads ≈ 0% and ALU ≈ 100% for A and B halves alike;
  - LLC bytes swing by ~1000× between equivalent passes.

  The counters don't discriminate the arms, so they are not reported as statistics.
- **Not tried:**
  - *Radeon GPU Analyzer* (offline, naga-emitted SPIR-V for gfx1010). It needs the owner's Windows machine, and it shows AMD's Vulkan compiler, not Metal's. That remains the cheapest way to confirm H-a's scratch spill.
  - *Xcode's interactive GPU frame capture.* It needs a GUI session.

## Stacked arm (F6)

Recommended levers: `array-free` with its `unroll36` modifier. `unroll36` cannot run without `array-free`, so the "sum of individual savings" is `array-free` alone plus `unroll36`'s increment over it. That equals the stacked measurement by construction.

| | Arena ind | Arena B | Station ind | Station B |
|---|---|---|---|---|
| Stacked (`array-free+unroll36`), measured | −2.23 (−33%) | −2.89 (−36%) | −2.13 (−27%) | −3.04 (−32%) |
| `array-free` alone | −1.55 | −2.91 | −1.50 | −2.85 |
| `unroll36` increment (stack − `array-free`) | −0.68 | +0.02 | −0.63 | −0.19 |
| Stacked on the `coalesced-b` layout (paired vs `coalesced-b`) | −2.20 | −2.82 | −2.18 | −3.23 |

Campaign spawn: −0.77 / −0.99 ms (−31% / −34%). Kinematic spawn: −2.19 / −3.05 ms.

## Projection through the contributing-row filter

The filter keeps only rows with entries. `T_post = c + r·rows_f + entry share`. The lever's post-filter saving is its per-row saving (stacked Δ / rows) × `rows_f`. This assumes rows with entries save what the average row saves. The entry work stays, and the lever does not touch it.

| Pose | Today ind / B (ms) | Filter alone ind / B | Lever on top of filter ind / B |
|---|---|---|---|
| Arena | 6.70 / 7.99 | 0.52 / 0.86 | −0.07 / −0.10 |
| Campaign spawn | 2.46 / 2.95 | 1.34 / 1.65 | −0.37 / −0.48 |
| Kinematic spawn | 8.08 / 9.49 | 2.77 / 0.88 | −0.66 / −0.09 |
| Kinematic station | 7.98 / 9.36 | 2.77 / 0.92 | −0.66 / −0.09 |

The lever keeps 15–25% of post-filter compose time on the Mac. That is 0.17 ms per frame at the arena and 0.85 ms at campaign spawn.

## Byte identity (H4, H5)

- **Coverage.** The capture dumps read back the composed `rgba16float` indirect and direct atlases, and are compared with the baseline dumps of commit `1ddd56691`. Scenes: three views (stress-warren-mini animroom, the hallway arena, kinematic station) × gated and force-full-resident × t = 0.5 s and 1.0 s, so 12 scenes.
- **Determinism.** The baseline repeats byte-identically across four probe binaries.
- **Results:**
  - **Every lever and every stack is byte-identical in all 12 scenes.** That covers `scan-parallel`, `scale-shared`, `texel-outer`, `array-free` (alone, `+unroll36`, `+l0-only`, both), `vec3-accum`, `const-tile`, `coalesced-b` and the stacks.
  - **Negative control:** `floor` differs in every scene, in both atlases. For example, at the arena (gated, t = 0.5 s) 24,523 indirect texels differ (max RGB deviation 0.083) and 51 direct texels differ.
  - **Oracle test.** `sampled_row_gate_capture_matches_full_resident_at_stepped_times` passes with each lever's env set: the first-round levers and stack, plus `array-free`, `array-free+unroll36`, `array-free+unroll36+l0-only`, `coalesced-b` and `coalesced-b+array-free+unroll36` (results in plan.md).
- **Coverage gap.** No fixture contains an L1/L2 row in ids 27/45, because the compiler never emits one. So `array-free`'s L1/L2 reconstruction path is exact by construction (same slot order and arithmetic), not proven by bytes.

## Recommendations (build calls)

| Lever | Label | Byte identity | Call |
|---|---|---|---|
| `array-free` + `unroll36`, both passes | **Metal-only** (no 1660 reading yet) | identical, 12/12 + oracle | **Build, via a brief** (see below) |
| `l0-only` | — | identical | No build: no gain over `array-free`, and exactness depends on L0-only content |
| `texel-outer`, `scale-shared`, `scan-parallel`, `const-tile` | — | identical | No build: they regress indirect |
| `vec3-accum` | — | identical | No build: no effect |
| `coalesced-b` | — | identical | No build: no gain beyond spread |

**Follow-on brief: `sh-compose-array-free`.** A brief rather than a direct build, because three questions need decisions:
1. **Indirect code-generation fragility.** Small edits move indirect by ±1–2.7 ms. The build should land the measured shape and carry a Mac paired A/B re-measure in its acceptance.
2. **Delete the L1/L2 path in 27/45, or prove it.** Either keep `array-free`'s L1/L2 reconstruction with a synthetic unit fixture, or decide that the loader rejects non-L0 levels in 27/45 and delete the path. That is a format-contract decision.
3. **Reading on the 1660.** A Metal-only lever still ships to the 1660 perf floor (§8 has no per-backend variants), so the follow-on must require a 1660 no-regression reading before landing.

Sequence it after the contributing-row filter. The filter is the 10× lever, and the post-filter rows are exactly the rows this lever's per-row saving still applies to.

## 1660 Super handoff (F8): pending

On the owner's Windows machine, with the probes branch at `1a052cfed`:

```
cargo run -p xtask -- run --release -- content/dev/maps/<map>.prl [--start-pose=…]
```

- **Arm per launch.** Set `POSTRETRO_SPIKE_ARMS=<arm>` per launch, unpaired. GPU timestamps work there, so the per-pass averages separate the arms.
- **GPU timing.** Set `POSTRETRO_GPU_TIMING=1 RUST_LOG=info`.
- **Arms:** `baseline`, `array-free`, `array-free,unroll36`.
- **Poses:** campaign-test spawn, and the hallway arena (`--start-pose=21.13,2.44,30.48,0,0`).
- **Protocol:** three launches each, interleaved. Read `sh_compose` and `animated_direct_sh_compose` from the `[GpuTiming]` window lines.
- **What it decides:** a regression in either pass makes the lever not recommended, and a win relabels it all-backends.
