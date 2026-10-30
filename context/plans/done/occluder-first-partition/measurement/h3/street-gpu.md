# occluder-first-partition: why stress-street's GPU rises (H3 follow-up)

Diagnosis only, 2026-10-28, HEAD `32d740dcb`. No source changed. The GPU was in the slowdown regime throughout (capture completion 200–232 ms per frame), so every claim below rests on bytes, counts and interval *frequencies*, not on milliseconds. Scratch tools and raw records: `$SCRATCH/street-gpu/` (session scratchpad): `run_up.sh`, `trace_up.sh`, `ivstat.py`, `updist.py`, `drawstat/` (a crate outside the workspace that calls the engine's own `determine_visible_cells` and reads the PRL's BVH leaves).

PRLs: before `target/occluder-baselines/stress-street/`, cap19 and cap26 `target/occluder-9g/<side>/stress-street/`. Poses: `poses/stress-street.poses`.

## First, a correction to the framing

The "+6% tris" H3 compared against is the street's **sweep-wide mean** (9c). At the two typical poses the traces measured, the drawn set rose far more. Engine walk plus BVH leaves of the drawn cells, which are exactly the slots the camera spans cover (`rendering_pipeline.md` §5):

| Pose | slots (driver draws), b → c26 | tris submitted | front-facing coverage, screens |
|---|---|---|---|
| `rd:elevated` | 1,395 → 3,343 (2.4×) | 2,704 → 6,098 (+126%) | 2.00 → 2.39 (+20%) |
| `rd:promenade` | 1,023 → 3,435 (3.4×) | 1,946 → 6,400 (+229%) | 1.63 → 1.91 (+17%) |

The `world_poses_ray_sweep` harness agrees: `drawn_faces` 1,395 → 3,343 and 1,023 → 3,435, `tris_in_frustum` 2,704 → 6,098 and 1,946 → 6,400. cap19 is within 12% of cap26 at both poses.

## Per-frame Upload Copies

**Bytes (`POSTRETRO_UPLOAD_TIMING=1`, frozen H3 capture binary, `rd:elevated`, steady-state 120-frame windows):**

| | writes per frame | bytes per frame |
|---|---|---|
| before | 10 | 45,868 |
| cap26 | 12 | 184,732 |

The +138,864 B is entirely accounted for:
- **+131,072 B: two 65,536 B SH compose grid windows** (`DYNAMIC_COMPOSE_GRID_DIMS_SIZE`). One is the streamed indirect compose (`sh_streaming/gpu/indirect/runtime.rs:261`) and one is the animated-direct compose (`sh_streaming/direct_compose/passes.rs:555`). Each fires once per frame on cap26 and never in steady state before. A firing pass writes one window per dispatch chunk, whatever its row count (here 4 rows).
- **+7,792 B: the candidate leaf list** (`candidate_cull.rs:504`), 4 B per candidate leaf. That is 1,948 more leaves, exactly the slot delta above.

No lightmap, residency or per-cell buffer uploads per frame on either side. Lightmap: 2 pool layers, 0 growths, 0 deferrals. SH: no reads after preload. There is no residency thrash.

**Why the compose passes fire.** The street's two script-pulsed lights sit at (−18, 4, −25) and (−30, 4, −25), about 70 m from these eyes. On cap26, the visible set at both poses includes eight 62 m lane cells, `x` −48…−40 and `z` −88…−26, stacked from 0 to 15.5 m (cells 3359, 3477, 3522, 3532, 3593, 3607, 3786, 3825). Their bounds, dilated 1.1 SH spacings for the sample region (§4 Sampled-row compose), come within 12 m of the lights. So the lights' delta rows enter the sampled rows, and both animated passes compose every frame. Before, no visible or fog-reachable cell comes near them. These cells exceed the 26 m horizontal cap: cap26 keeps 43 interior cells over 27 m horizontal, and before had 64. This diagnosis did not identify which `cell_cap.rs` guard leaves them whole.

**Why 184 KB costs 4–9 ms.** It doesn't, as copy work. In fresh traces (in the regime, counts only):

| `rd:elevated` | copy interval p50 | frames with the interval over 1 ms | pager (SDMA1_PAGE "GPU Execution") per frame |
|---|---|---|---|
| before | 0.10 ms | 9 of 346 (2.6%) | 0.48 ms |
| cap26 | 0.17 ms | 129 of 288 (45%) | 3.74 ms |

The long intervals run 1–18.6 ms on SDMA0 and overlap the pager channel's activity (3.5 of 3.8 ms per frame, mean). Page-on volume per frame rises by 131,712 B (1,744,425 → 1,876,137), which is the two grid windows. So the cost is the DMA copy stalling behind driver paging. It is not bytes.

The driver mechanism was not identified. The stall follows the compose passes firing on the street. The before build pays it too, wherever they fire there:
- `st:g5.3.0@180` (a trip) before: 6.77 ms;
- campaign-test before: 5.18 ms;
- `x:lights@0` (see below).

It does not appear when compose fires on occlusion-test, hallway or arena (0.15 ms or less), nor at `roof:B3` before (compose 0.52 ms). **This is a latent renderer or driver interaction that the new cells expose at typical street poses, not a cost the partition creates.**

`x:lights@0` (eye −24, 1.5, −40, yaw 0; before, a portal walk with no trip) composes every frame on before: 13 writes, 181,640 B per frame. Its traced upload copy stretches past 1 ms in **150 of 341 frames (44%)**, with a p50 of 0.14 ms and a mean of 1.58 ms, overlapping the pager by 1.2 ms per frame. That is the same pattern as cap26 at `rd:elevated`, on the before build. **This confirms the stall is latent:** it comes with the street's compose firing, and the partition only moves that firing into typical poses.

## Depth Pre-Pass

The depth pre-pass draws the same span list as forward. Each slot is one Metal driver draw (§5: multi-draw of N expands to N). Its rise tracks **slots and triangles, not coverage**: coverage rises 17–20%, slots 2.4–3.4×.

Where the slots come from at cap26:
- **Facade slab cells drawn whole.** Interior cells under 0.5 m thick hold 1,295 of `rd:elevated`'s 3,343 slots and 1,540 of `rd:promenade`'s 3,435, with 0.19–0.23 screens of coverage. Before, they held 658 and 493. These are window-opening air inside strip facades: 0.1–0.4 m thick, 14 m tall and 24–38 m long, holding 104–252 leaves each (every window's jambs, sills and lintels). Seeing one window draws them all. Map-wide, the cells under 0.5 m thick go from 934 cells holding 3,297 leaves to 422 holding 4,574. The cap is horizontal only, and the 38.4 m slab cells at `x` −30.4 also pass the 26 m cap.
- **Fragments and larger cells.** Submitted leaves under 0.25 m² go from 70 → 626 (elevated) and 39 → 826 (promenade). Map faces go from 7,657 → 10,441.

*Inferred:* the per-draw cost is what grows. In the pre-regime H3 traces, forward pays an increase on the same scale (Textured +2.7–3.4 ms, against depth's +1.4–1.8 ms). Depth's 6–9× ratio comes from its tiny base. At or under about 1,400 slots, depth stays at 0.15–0.35 ms; that covers every traced pose without these cells (9–1,415 slots).

## Why the street only

- **Strip facades** exist only on the street, by design (the street-map Decision). Their in-slab cells concentrate hundreds of leaves in cells visible through any window.
- **Pulse lights**, plus lane cells the cap leaves 62 m long, put animated SH rows into typical poses' sample regions, which triggers the latent upload stall.
- The other maps' traced poses draw 164–799 slots and compose without stalling.

## The lever

1. **Compiler, smallest and first: split the oversize street cells.** Find why `cell_cap.rs` leaves the 62 m lane cells whole, and cap them. *Estimate (inferred):* at both poses, the lane's far end near `z` −26 lies 75–83° off the view axis against a 50° half-FOV. Lane pieces near the lights would therefore leave the frustum, the compose passes and their 128 KiB of uploads stop, and with them the stall. That is roughly −1.8 ms of compute plus up to 4–9 ms of copy interval at these poses. It is unconfirmed until rebaked.
2. **Compiler: bound the facade slab cells.** Divide them per window column or storey, or let the cap cut them vertically. That removes up to about 1,300 of the 3,343–3,435 slots. The rest is `face-fragment-remerge` (already named) and larger cells generally, so depth stays above before (about 2,000 slots against 1,000–1,400).
3. **Renderer, F3-frozen, for an owner ruling:**
   - the upload stall behind paging whenever SH compose fires on the street, latent on before;
   - Metal's per-slot draw cost over every leaf of a drawn cell.

   Neither is the partition's bug, but both shape what "no significant increase" can mean on this card.
