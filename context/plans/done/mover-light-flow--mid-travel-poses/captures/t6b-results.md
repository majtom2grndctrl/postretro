# Task 6b: mid poses trace their own later bounces, results

Measured 2026-11-10 on the AMD Radeon Pro 5300M under Metal (Intel i9 MacBook Pro, `-j 6` under `nice`), on this branch at 57e0f44e7 plus the task 6b working tree. A mid pose now re-bakes bounces one through its `light_bounces` posed and keeps id 34's past them. Its later bounces gather patch to patch over the static patches whose centre lies in the near-field domain and over its posed sides; every other static patch holds id 34's own bounce at each step, so a ray leaving the domain reads the mover-absent bake. The termini-blended later shares are gone.

## Builds

| Column | Compiler and engine | Notes |
|---|---|---|
| today | 83dfb5317 (epoch 4) | task 5's captures |
| task 5 | task 5's build, through-base | task 5's captures (`tb`) |
| task 6a | 57e0f44e7 | `t6a-results.md` |
| task 6b | this tree's cargo `--release` `prl-build`; engine `postretro-t6a` | `prl-build --release --no-tui -v -j 6`, cold. The engine binary is task 6a's: task 6b changes only the compiler |
| reference | task 5b's static-brush reference through its CPU model (`t5b/dump/base/room-cpu.json`) | no Chebyshev depth, no dynamic lights; reads about 1 luma under the GPU on the same data |

- Harness: `t6b-harness/cap6b.py` (task 6a's driver, column `t6b`), `t6b-harness/charts6b.py`. Raw metrics: `t6b-harness/t6b-results.json`. Chart: `t6b-luma.png`.
- Only id 57's bounce block changed. Every other section's size equals task 6a's on both maps; the direct block, the depth block and the across marks are byte-for-byte the same size.

## Bytes and cost

| | setpiece, task 6a | setpiece, task 6b | enemy-movers, task 6a | enemy-movers, task 6b |
|---|---|---|---|---|
| id 57 | 11,347,295 B | 11,292,647 B (−54,648) | 4,552,318 B | 4,596,382 B (+44,064) |
| bounce payload | 4,845,744 B | 4,791,096 B | 2,099,736 B | 2,143,800 B |
| cold `--release`, wall | 20.4 s | 23.6 s | 6.8 s | 7.7 s |
| SH Bake | 7.22 s | 10.44 s | 5.57 s | 6.47 s |

Per mover and per pose (setpiece, cold `--release`, `RUST_LOG=prl_build::mover_probe_channel=debug`):

| Mover | Poses | Task 6a | Task 6b | Each terminus | Each mid pose: bounce one / later bounces / gather |
|---|---|---|---|---|---|
| `setpiece_wall` | 15 (13 mid) | 3.1 s | 5.3 s | 1.15–1.20 s | 0.02 / 0.17 s (4 bounces over 2,692 of 20,659 static + 1,102 posed patches) / 0.03 s (588 probes) |
| `setpiece_room` | 6 (4 mid) | 3.0 s | 4.1 s | 1.31–1.40 s | 0.01 / 0.25–0.26 s (1,251 static + 2,470–2,546 posed) / 0.05 s (856 probes) |

On enemy-movers each terminus takes 0.52–0.55 s and each mid pose 0.07–0.10 s (later bounces over 210–494 of 10,982 static patches).

**The mover-absent later bounces** are id 34's own four later bounces, one front side per static patch.
- Cold: no extra trace. The SH base's own later-bounce pass keeps them (`kept for the movers from its own bake`), at 48 B per static patch: 0.99 MB on the setpiece.
- Warm, id-34 hit, memo hit: read from `mover_absent_bounces`, no trace.
- Warm, id-34 hit, memo miss: one later-bounce pass, 0.6 s on the setpiece (logged `re-traced for the movers`), then memoized.

Warm series on the setpiece (one cache, interactive, `-j 6`):

| Run | Wall | Cache lines |
|---|---|---|
| cold, cache written | 24.4 s | volume miss; both movers re-bake; absent bounces kept from id 34's bake |
| warm, no edit | 0.94 s | volume hit; both movers hit |
| warm, `setpiece_room` `light_bounces 4` | 4.3 s | volume hit; 1 of 2 movers re-bakes; `mover_absent_bounces hit` |
| warm on a cache primed by `light_bounces 0` movers (absent bounces never stored), both movers re-bake | 11.2 s | volume hit; `mover_absent_bounces miss`; re-traced 0.6 s |

**Hallway, projected, not baked** (`stress-warren-hallway-inspection`, 1,748,534 patches, 11 movers; probe-channel research §M1):
- Absent bounces: free cold; 84 MB held and memoized (4 × 12 B × 1.75 M). A warm build with an id-34 hit and a memo miss re-traces them once: about one id-34 later-bounce pass, ~130 s by the brief's figure. Every later warm build on that id-34 key reads the memo.
- Termini: unchanged by this task, ~216 s each (22 termini ≈ 4,750 s cold).
- Each mid pose: about 70 µs per gathered patch for four later bounces (M1: 130 s / 1.75 M patches = 74 µs; here 45–68 µs per static-plus-posed patch), plus about 60 µs per domain probe of gather. A door whose domain holds ~3,000 static patches, ~1,000 posed and ~500 probes: ~0.3 s a mid pose. A 50 m lift at the 16-pose cap, with ~30,000 domain patches, ~2,000 posed and ~3,000 probes: ~2.5 s a mid pose, ~35 s for its 14. The domain sizes are assumptions; the hallway's were not measured.

## Rising room interior mean luma

Chart `t6b-luma.png`. t′ every 0.025; selected rows:

| t′ | today | task 6a | task 6b | reference (CPU) |
|---|---|---|---|---|
| 0.10 | 1.66 | 1.28 | 1.20 | 0.21 |
| 0.15 | 3.74 | 2.44 | 2.01 | 0.13 |
| 0.20 | 4.12 | 1.92 | 1.23 | 0.34 |
| 0.30 | 4.77 | 2.39 | 1.76 | 0.27 |
| 0.40 | 5.30 | 2.96 | 2.43 | 1.51 |
| 0.50 | 5.65 | 3.21 | 2.75 | 3.01 |
| 0.55 | 5.77 | 3.31 | 2.88 | 4.13 |
| 0.60 | 6.55 | 5.05 | 4.88 | 3.31 |
| 0.70 | 6.52 | 6.69 | 6.67 | 5.13 |
| 0.80 | 12.82 | 7.15 | 6.80 | 5.70 |
| 0.90 | 12.19 | 8.02 | 7.73 | 7.29 |
| 1.00 | 9.08 | 9.08 | 9.08 | 8.59 |

- Darker than task 6a everywhere mid-travel, by 0.4–0.7 luma over 0.15–0.55 and 0.2–0.4 over 0.8–0.95.
- At the knots it sits 0.9 (t′ 0.2, 0.4) and 1.6 (t′ 0.6) luma over the CPU reference, about the reference model's own ~1 luma offset against the GPU.
- **Bounce inside the room is now within the bar at the knots.** CPU compose against the static-brush reference over the 224 probes inside the room's walls, by AC 19's harness with an inside group (a temporary run, not kept):
  - t′ 0.4: worst 0.33, mean 0.08 levels.
  - t′ 0.6: worst 0.53, mean 0.11 levels.
  - Task 5b measured 11.55 / 4.93 and 8.88 / 5.06 (worst / mean) with the termini-blended shares.
  - t′ 0.3, between knots: bounce worst 2.07, mean 0.28. Direct worst 49.6 levels there, the piecewise direct blend across a probe layer's crossing.
- What remains between knots is the direct half's blend and the reference model's offset, not later bounces.
- The knot dips (t′ 0.2, 0.4) remain, about 0.5 luma, as at task 6a.

## Sweeper faces (12d: as built minus stripped, block means)

Cells: crop mean, then +bright max / peaks, −dark max / pits.

| Face, t′ | task 6a | task 6b |
|---|---|---|
| west 0.20 | −0.6, +0.5/0 −1.0/0 | −0.6, +0.5/0 −1.0/0 |
| west 0.25 | −1.8, +1.0/0 −1.9/0 | −2.2, +1.0/0 −1.8/0 |
| west 0.28 | −0.2, +0.8/0 −1.5/0 | −0.6, +1.0/0 −1.4/0 |
| west 0.30 | +0.2, +1.0/0 −1.3/0 | −0.2, +1.1/0 −1.2/0 |
| west 0.32 | +0.2, +0.6/0 −1.0/0 | −0.2, +0.7/0 −1.0/0 |
| east 0.00 | −7.1, +2.3/0 −7.6/12 | same |
| east 0.05 | −4.3, +2.2/0 −6.7/10 | −3.7, +1.9/0 −6.4/8 |
| east 0.10 | −3.5, +2.0/0 −4.0/9 | −3.0, +1.9/0 −4.1/10 |
| east 0.143 | −5.9, +1.6/0 −5.4/8 | −5.9, +1.5/0 −5.4/9 |
| east 0.15 | −5.0, +1.5/0 −5.3/10 | −4.8, +1.5/0 −5.4/11 |
| east 0.20 | −2.8, +1.6/0 −4.6/2 | −2.1, +1.4/0 −4.6/3 |
| east 0.25 | −2.8, +2.2/0 −2.7/0 | −2.2, +2.1/0 −2.7/0 |
| east 0.70 | −3.6, +1.5/0 −1.5/0 | −5.8, +1.9/0 −1.9/0 |
| east 0.75 | −5.1, +3.0/0 −2.1/0 | −5.3, +2.5/0 −1.9/0 |
| east 0.80 | −5.8, +5.0/12 −2.9/0 | −5.8, +4.6/13 −2.8/0 |
| east 0.85 | −6.6, +4.2/9 −3.4/3 | −6.5, +3.9/8 −3.4/3 |

- West face: within 2.2 levels of stripped with no pits at 0.20–0.32.
- East face: within 0.7 of task 6a everywhere but t′ 0.70, which reads 2.2 darker. Stripped is not the truth on this face (task 5b Question 2), and no new reference dump was taken for it.

## AC 19, AC 7 and AC 8 (compiler, against static-brush references)

AC 19 (`mid_pose_compose`), CPU compose from the assembled id-57 section:

| Group | Probes | Task 6a: bounce / direct | Task 6b: bounce / direct | Termini only |
|---|---|---|---|---|
| em_sweeper, t′ 0.30 | 20 | 4.19 / 0.00 | **1.07** / 0.00 | 19.54 / 58.57 |
| roof, z 83.6, t′ 0.4 | 64 | 3.57 / 0.00 | **1.78** / 0.00 | 11.09 / 23.34 |
| roof, z 123.0, t′ 0.6 | 64 | 6.04 / 0.00 | **1.80** / 0.00 | 14.51 / 39.70 |

Both tests pass and now run in the suite (23 s for the pair).

AC 7 (bounce one) and AC 8 (full bounce) at the middle mid pose. Probes within one patch of a posed face off the 0.5 m patch grid are set aside (AC 7's exception):

| Mover | Bounce one worst | Full bounce worst | Set aside: count, worst one / full | Direct worst | Depth |
|---|---|---|---|---|---|
| setpiece room, pose 3 of 6 | 0.87 levels (474 of 480 within the BC6H bar) | 1.92 (391 of 480) | 286, 0.87 / 2.41 | 0.50 q steps | 286 tiles, 0 past f16 |
| em_sweeper, pose 4 of 8 | 2.30 (250 of 264) | 2.83 (173 of 264) | 40, 4.69 / 5.94 | 0.44 q steps | 23 tiles, 0 past f16 |

Both pass the three-level bar. The room passes even without the exception. The sweeper needs it: its set-aside column reads up to 5.94.
