# Task 15c: final measurement (HEAD fff783e14: 1 u outside knots + GPU knot bracket)

Release `prl-build`, `postretro --features capture` and `scripts-build` were built at `fff783e14` after touching the crate roots, and copied to scratchpad `t15/bin-final/` (hashes in `tree.txt`). The tree held another agent's uncommitted edits: docs, the FGD, the ip-372 note, and a test-only string in `parse.rs`. None of them reaches a bake or the engine.

## Bakes (`--release`, cold, quiet machine)

| Map | Cold bake | id 57 | direct | bounce | depth | Closed-pose tiles |
|---|---|---|---|---|---|---|
| mover-setpiece | 44.3 s | 12,030,273 B | 3,165,433 | 4,602,268 | 4,262,056 | 10,584 |
| enemy-movers | 14.5 s | 4,515,849 B | 668,445 | 2,021,172 | 1,825,720 | 4,530 |

Pose counts (even + geometry):
- setpiece wall 71 (15 + 56), room 45 (5 + 40);
- em_lift 11 (3 + 8), em_sweeper 31 (7 + 24), em_crusher_open 26 (6 + 20), em_crusher_wall 39 (9 + 30).

Against task 15 (4.4 u):
- setpiece 39.1 → 44.3 s, id 57 11.39 → 12.03 MB;
- enemy-movers 13.6 → 14.5 s, 4.35 → 4.52 MB.
The M7 variants' id 57 is byte-identical to this bake.

## Motion (M9 record)

The five views on the task 15 grids, column `final` (595 new frames), beside `head`, `two` and `truth`. Truth means the `light_poses` 41 / 57 bakes at their own knots.

| View | mean \|final − truth\| | max (t′) | mean \|head − truth\| | per-cell max, final / head |
|---|---|---|---|---|
| ceiling | 0.09 | 0.45 (0.475) | 4.41 | 5.57 / 30.97 |
| floor | 0.10 | 0.68 (0.375) | 1.19 | 11.26 / 23.23 |
| interior | 0.05 | 0.37 (0.825) | 0.40 | 10.83 / 23.40 |
| east face | 0.58 | 3.69 (0.161) | 3.87 | 15.18 (0.982) / 17.90 |
| west face | 0.08 | 0.37 (0.232) | 0.86 | 1.38 / 7.05 |

- **Ceiling knot flash:** over the room's 43 mid knots, the mean drop against ±0.001 t′ is 0.05 on average and 0.38 at worst (knot 0.116). Pre-amendment it was 5–6.
- **Ceiling 12c peaks between knots** (median / max): final 9 / 27, head 24 / 36, truth 1 / 24. A faint lattice remains.
- **East-face peak-to-trough** per 1/7 period over t′ ≤ 4/7: final 8.3 / 5.5 / 2.1 / 1.0, truth 8.6 / 4.1 / 1.5 / 0.9, head 7.7 / 10.7 / 9.9 / 6.2. The sawtooth is gone; what remains is the truth's own crossing drops. Residual against truth over t′ ≤ 0.6 spans 5.3 (head 13.1).
- **East-face misses that remain** are at +3.7 (t′ 0.161) and −2.0 (t′ 0.98, the last 0.8 m interval). Both are long-interval direct interpolation, which the pre-amendment build shares (task 15b).
- **Steps on the truth grid** match the truth's in every view: ceiling 9.3 / 9.5, floor 34.3 / 34.4, west 10.9 / 11.0.

## M7: GPU frame cost, HEAD `fff783e14` against `83df`

The same harness and settings as task 15:
- Metal System Trace, windowed 2560×1440, async streaming, core clock median 1228–1229 MHz, die temperature median 83–85 °C.
- Interleaved final r1 → 83df r1 → … → 83df r3; 3 valid rounds per config per build; the pilot ran one run per build first.
- Two docked-r3 attempts were excluded for foreign CPU spikes (contactsd/accountsd; the Claude renderer). Each retry was valid.
- Traces were deleted after parsing.

Figures are per mover, minus docked, in ms (medians of 3):

| build | mover | bounce | Pass A | uploads | sum | live-tile writes / 120 frames |
|---|---|---|---|---|---|---|
| final (HEAD) | room | +0.987 | +0.619 | +0.117 | +1.722 | 2748.5 |
| final (HEAD) | wall | +1.392 | +1.082 | +0.097 | +2.570 | 960 |
| final (HEAD) | both | +0.824 | +0.550 | +0.069 | +1.443 | 2509.5 |
| 83df (rerun) | room | +1.000 | +0.597 | +0.053 | +1.641 | 2213.5 |
| 83df (rerun) | wall | +1.332 | +1.194 | +0.008 | +2.535 | 936 |
| 83df (rerun) | both | +0.838 | +0.658 | +0.033 | +1.534 | 2023 |
| task 15 HEAD | wall | +1.545 | +1.268 | +0.064 | +2.877 | 960 |
| task 15 83df | wall | +1.422 | +1.253 | +0.073 | +2.748 | 952 |

Pass B is +0.000 everywhere. The full table, task 15 rows included, is `t15c-m7-ab-table.md`.

Per-run compose (bounce + Pass A + Pass B), absolute ms:

| config | final (HEAD) | 83df rerun | task 15 HEAD | task 15 83df |
|---|---|---|---|---|
| wall | 2.456 / 2.472 / 2.477 | 2.509 / 2.527 / 2.608 | 2.756 / 2.813 / 2.833 | 2.618 / 2.675 / 2.724 |
| room | 1.536 / 1.606 / 1.624 | 1.398 / 1.597 / 1.624 | 1.683 / 1.698 / 1.760 | 1.425 / 1.538 / 1.685 |
| all | 2.723 / 2.748 / 2.761 | 2.926 / 2.993 / 3.002 | 2.979 / 3.091 / 3.139 | 3.023 / 3.052 / 3.182 |

**Verdict: the wall's +0.13 ms compose is gone.**
- **Wall:** in task 15, HEAD's compose sat above 83df's range by +0.03 to +0.11 ms (medians 2.813 against 2.675). With the bracket, all three final runs fall below the rerun's 83df range: 2.456–2.477 against 2.509–2.608, a median −0.06 ms. Pass A drops 0.11 ms per mover. With uploads included, the wall sum is +0.035 ms over 83df. That is within run spread, and the gap comes from the frame upload (+0.09 ms), not compose.
- **Room:** inside 83df's range (1.536–1.624 against 1.398–1.624). Uploads run +0.06 ms higher, in line with 24 % more live-tile writes from the room's 45 poses.
- **Both:** below 83df: compose 2.72–2.76 against 2.93–3.00, sum per mover −0.09 ms.
- **Session drift:** 83df's own wall sum moved 2.748 → 2.535 between sessions. Only the interleaved pairs compare like with like.
