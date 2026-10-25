# portal-walk-bounded-regions: independent assessment, 2026-10-25

Read at HEAD `fcb34711c` (branch `planar-water-reflections`). Uncommitted, read-only review. Labels: **[src]** checked in source at HEAD, **[meas]** measured in this session, **[doc]** cited from a context doc or plan, **[inf]** my inference.

## Verdict: promote after the edits listed below

The problem is real at HEAD, and it is wider than the brief says. The rect region is the established fix and the right one. The Decisions' premises hold in source, with one stale dependency line. Three things stand in the way of promotion:

1. The seeded entry that planar-water-reflections is blocked on appears only in Path, so nothing binds the build to deliver it.
2. The evidence and sweep cover only the hallway. The lake and occlusion-test maps trip the cap more often and show looser rect behaviour.
3. The residency gate's baseline is biased, and the manual block is too big for a compact brief. Either relabel the brief resumable or move the instrument work out.

None of this changes the direction, so the brief does not need rework.

## Measurements run

Machine: Intel i9-9980HK (8 cores, 16 threads, 2.4 GHz) with an AMD Radeon Pro 5300M (4 GB), macOS Darwin 25.6.0, Metal. Scratch outputs are in `/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/62ffc664-8f32-4400-834e-75c54e88f5b3/scratchpad/pwbr-assessment/`.

**Prototype at HEAD.** I copied `prototype/` to scratch and changed three things only:
- pointed the Cargo paths at this checkout;
- moved `glam` from 0.32 to 0.33 to match the workspace;
- added the `TimingGate::OFF` argument, which `determine_visible_cells` now takes.

The prototype's walk copy matches the engine's `flood`. The only difference is the epsilon-sliver experiment, which is off by default. I built it with `--release`. The analysis script crashes on maps that lack the hallway's named regions, so I patched it to skip empty groups (`analyze_alt_any.py`).

The exact sweep runs on 16 threads, so its µs figures are inflated and I quote only its step counts. The alt sweep is single-threaded, best of 2 runs.

| map (current local `.prl`) | walks | exact trips at 20k | exact steps p99 / max | rect256 steps max | rect µs p50/p99/max | rect extra cells p95/max (no bypass) | rect misses vs exact (poses) |
|---|---|---|---|---|---|---|---|
| stress-warren-hallway-inspection | 94,440 | **103 (0.11%)** | 6,722 / 37,139 | 1,922 | 2.8/65/303 | 3/71 | 0 |
| reflection-lake-test | 5,208 | **203 (3.9%)**; 170 of 1,736 at pitch 0 | 27,793 / 45,064 | 1,533 | 17.9/117/309 | 4/50 | 4 |
| occlusion-test | 13,128 | **394 (3.0%)** | 35,892 / 75,774 | 1,987 | 31/177/378 | **21/182** | 2 |
| stress-env-volumes | 151,920 | 134 (0.09%) | 8,462 / 54,407 | 1,554 | 2.9/74/252 | 3/65 | 8 |
| reflection-probes-test | 3,288 | 5 (0.15%) | 5,495 / 105,987 | 666 | 8.2/42/62 | 2/13 | 0 |
| movement-feel | 2,736 | 0 | 8,662 / 12,967 | 937 | 4.9/54/128 | 2/28 | 0 |
| campaign-test | 8,928 | 0 | 2,165 / 4,028 | 601 | 5.5/32/109 | 2/24 | 24 |
| e24-environment-test | 3,840 | 0 | 982 / 1,846 | 369 | 4.5/24/45 | 2/14 | 0 |

- **Hallway, reproduced exactly [meas].** Same 103 trips over 32 cells, same exact and rect step percentiles, same 92-cell worst extra. The cross-check found 0 mismatches against `postretro_visibility::determine_visible_cells` over 2,010 walks.
- **Live lake run [meas].** `reflection-lake-test` from its map spawn, untouched, under `POSTRETRO_CPU_TIMING=1`, `dev` profile with `dev-tools`, VSync on, screen saver checked (off).
  - Over 33 windows of 120 frames each, `walk_step_limit` was 120/120 in every window.
  - `portal_walk` averaged 3.56–3.81 ms (max 5.2 ms) out of 10.2–11.1 ms of work CPU, about 35%. The walk hit `walk_considered` = 20,000 every frame.
  - `settle_hold` was 1,040 ms.
  - This agrees with planar's measurement of ~2.9 ms of a ~5 ms frame on the GTX 1660.
- **Not run.**
  - A live hallway shaft-A run. It needs the 3.3 GB `.prl`, and the offline cross-check already ties the sweep to the engine function.
  - Any release-profile live run.
  - The residency, settle and SH-reach manual rows. Their instruments don't exist yet.

## 1. Is the problem real at current HEAD?

Yes, and it is broader than the brief states.

- **Nothing in the walk has changed since the brief's read commit [src].** `git log 27ca6a6..HEAD -- crates/visibility` is empty. A pickaxe search since 27ca6a6 for `determine_visible_cells`, `PortalStepLimitFallback`, `visible_cells_frustum_all`, `fog_reachable_frustum_fallback`, `record_walk` and `walk_cube_faces` finds only commits that touched plan docs (planar, teleporters), no source changes.
- **The cap and epoch are where the brief says.**
  - `MAX_PORTAL_WALK_STEPS = 20_000` (`portal_vis.rs:26`).
  - `PORTAL_WALK_EPOCH = 1` (`lib.rs:14`).
  - The fallback still draws `visible_cells_frustum_all`, with fog reach from `fog_reachable_frustum_fallback` (`visibility.rs:435-470`).
- **One change since then touches the bake [src].** Teleporters (86bec6c83, f7784d600) added routes to `cell_residency_bake`. `CELL_RESIDENCY_SET_STAGE_VERSION` is now 3 (`cell_residency_bake.rs:46`). The walk sampling in `pvs_sampling::walk_cube_faces` is unchanged. This doesn't affect the brief, beyond its "stage version matches its value before" row reading 3.
- **The hallway stair room still trips the cap [meas].** 103 of 94,440 walks, all around shaft A.
- **The brief's Problem undersells it.**
  - The lake trips on about 10% of pitch-0 walks and on every frame at its spawn [meas].
  - On the lake the cost is the walk itself, not fallback overdraw: frustum-all draws only 1.1× the exact set at p50 there. The brief's causal sentence ("the bounded fallback then draws several times the visible cells") describes the hallway only.
  - occlusion-test trips on 3% of walks.
  - reflection-lake-test is planar's primary map, so it belongs in the evidence and in the manual sweep.

## 2. Is the direction right?

Yes.

- **It is the established bounded answer [doc, inf].**
  - Descent re-queues a segment's window when it grows.
  - Source unions a per-area screen rect (`R_FlowThroughArea`).
  - Thief uses an octagon; Umbra uses a coverage buffer.
  - Doom 3's chain DFS, today's walk, is the outlier. It works because Doom 3's portals are hand-placed.
  - Quake/Quake 3 PVS is ruled out by the owner (§2 and the no-PVS memory).
  - Luebke & Georges cull boxes are rects intersected along each chain. They have no per-cell union, so their cost is still path-shaped.
- **"Per-cell visited state with frustum union" is not a missing alternative [inf].** A union of convex frusta isn't convex. Storing it exactly is the chain enumeration again, and bounding it is the region approach. A screen rect is exactly a 4-plane frustum through the eye, so the rect is the frustum-union option with a bounded representation.
- **Rect vs octagon.**
  - My numbers reproduce the research: oct256clip costs about 1.15–1.4× rect's steps and µs.
  - It saves almost nothing on the loosest content. On occlusion-test, p95 extra is 19 for the octagon vs 22 for rect.
  - Rect is the right default. Keeping the region behind one type is right.
- **Restart-hybrid is the only serious rival.** It runs exact to a budget, then reruns as rect on a trip.
  - It is tighter: p95 extra 0 on most maps.
  - It costs about 2× rect's p99 µs, and the id-51 bake would sample an exact walk on some poses and a rect walk on others.
  - The brief's rejection holds.
- **The superset contract is safe for every consumer, and better than today where it matters [src, meas].**
  - No consumer reads a visible cell as line of sight; the research's consumer table checks out against source spots I sampled: `candidate_cull.rs:71` treats the fallback as eligible, and `cell_demand.rs:109-118` maps paths to demand.
  - At every pose that trips today, consumers already receive the frustum-all superset: median 345 vs 77 cells on the hallway. Shadow ranking, the mesh cap, SH Visible demand and fog reach all see that set.
  - The rect walk hands them a smaller set at exactly those poses, and adds p95 2–4 cells elsewhere.
  - The one real regression is looser content. occlusion-test has p95 +21 and max +182 extra cells even without the bypass. That is where shadow-slot and mesh-cap displacement and never-refused demand will show first. The brief should sweep it.
- **A finding on the oracle [meas, src].** Today's camera-on-plane bypass feeds the full unclipped polygon to `narrow_frustum` (`portal_vis.rs:411-416`). `narrow_frustum` keeps only the portal plane, the edge planes and the far plane (`portal_vis.rs:693-767`), not the parent frustum's side planes. So after a bypass the exact walk can flood outside the view frustum.
  - The campaign-test misses replay this way (walk 4898, target 278). The chain starts at a bypass hop (`s_src=0.00000`) and every later clipped polygon sits at NDC x ≈ 1.6–3.3, off screen.
  - The lake miss I replayed (walk 3831, target 149) is the brief's other class: a 3-vertex sliver chain, with the final polygon at NDC x −1.16..−1.08.
  - Both fall under the brief's "final clipped polygon wholly outside the view frustum" exception.
  - The test-only oracle should still intersect the bypass's narrowed frustum with the parent frustum. Otherwise the "unclassified exception fails" row may trip on chains that leave the screen and re-enter.

## 3. Are the Decisions sound, and are their premises true in source?

Every cited seam exists and behaves as described:

| Symbol | Where | Check |
|---|---|---|
| `flood` | `portal_vis.rs:281` | Recursive per-chain DFS. The only guards are the portal-path cycle check, `MAX_PORTAL_CHAIN_DEPTH` (256) and the step budget. No per-cell state. ✔ [src] |
| `portal_traverse_detailed` | `:86` | Wraps `portal_traverse_with_step_limit` at the 20k cap. ✔ |
| `clip_polygon_to_frustum` | `:493` | Sutherland-Hodgman with Doom 3's ON rule. ✔ |
| `narrow_frustum` | `:693` (pub) | Portal plane, edge planes and far plane only; see the §2 finding. ✔ |
| `camera_on_polygon_plane` / `APEX_ON_PORTAL_PLANE_EPSILON` | `:671` / `:667` (1e-3) | Tests distance to the infinite plane through the centroid, not to the polygon. ✔ The Decision's premise is true. |
| `VisibilityPath::PortalStepLimitFallback` | `visibility.rs:94` | ✔ The fallback is bounded. |
| `PORTAL_WALK_EPOCH` | `lib.rs:14` = 1 | Hashed into the id-51 key at `cell_residency_bake.rs:251`. ✔ |
| `cpu_stages::record_walk` | `cpu_stages.rs:89` | Records walk ns, every counter and the StepLimit marker. ✔ |
| `VisibilityStats` | `visibility.rs:38` | The title bar reads `walk_reach()` (`frame_loop/mod.rs:2713-2740`). ✔ |
| `cell_residency_bake::pvs_sampling` | `pvs_sampling.rs` | "Lower bound" and "any runtime FOV" appear at :10-17 and :301. The step-limit branch re-runs the capped `portal_traverse` (:296-306). The dry-run test pins "lower bound on true visibility" (`visible_set_tests.rs:267`). `tiles_render.rs:59` also says "lower bound", inside the grep gate's tree. ✔ |
| `walk_measurement` | `walk_measurement/mod.rs:57-58,158-163` | HFOV 100° and 16:9 are constants. Pitch is fixed at 0 implicitly, because the look vector has y = 0. Frames sleep to a 16.7 ms deadline (:527-531). ✔ |

Problems with the Decisions:

- **The dependency Decision is stale.** `sh-streaming--pool-growth-device-limit` is in `plans/done/` (c63ddd09a, an ancestor of HEAD), not `ready/` [src]. `AtlasShape::grown_for_slots` now clamps to `max_texture_array_layers` and returns `GpuDeviceLimit` only when the minimum doesn't fit (`renderer/src/render/sh_streaming/gpu.rs:168-196`). Refusal is per cluster (`targeting.rs` `DeviceLimited`, `controller_refusal_tests.rs`). Restate the Decision as satisfied, and drop O20 along with the manual preamble sentence that cites it.
- **The Path's claim that the `walk_measurement` "SH does not stream" note is stale is wrong.** The note describes the harness, which passes `no_sh = None` (`:364`). It is accurate; drop that Path line.
- **The "Placement" Decision is silent on hot-path allocation.** It says consumers keep their shape, but the rect walk needs per-caller scratch: per-cell rect, stamp and queue state. That scratch must survive between frames and stay safe across rayon bake threads (O7). Today's walk allocates `vec![false; cell_count]` every frame, so allocation is no regression. Still, `development_guide.md` §1.4 asks a plan to name the hot-path constraint. Either thread scratch through `determine_visible_cells` (the callers are the frame loop, the bake, `walk_measurement` and the probes), or say "allocation no worse than today" [inf].
- **The bypass, epoch and bake-gap Decisions are sound.**

## 4. Is the Acceptance list achievable, and are its rows sound metrics?

The list holds 32 automated rows and 6 manual rows. Most automated rows are sound and cheap on synthetic fixtures.

**Missing rows**

- **The seeded entry.**
  - §2 already says the mirror "floods from its seed cells through the same walk" [doc]. Planar's Decision depends on it, and planar's plan is blocked on it [doc].
  - Here it sits only in Path, which is non-binding.
  - The spike's `portal_traverse_seeded` (8201dfb8e, `portal_vis.rs:137`) differs from the camera entry in three ways:
    - it takes several seed cells and no apex cell;
    - it takes a caller-supplied oblique water near plane, which must not slide to the mirrored eye;
    - all seeds share one cap.
  - Add a Decision: one walk core with two entries, camera (one seed, full rect, near plane slid) and seeded (seed cells, caller frustum unslid).
  - Add two rows:
    - The seeded walk is a superset of a seeded oracle on the slab and lattice fixtures, never trips there, and shares the cap.
    - A one-seed call at the camera cell equals the camera entry.
- **The other maps.** Add reflection-lake-test and occlusion-test to the manual sweep (zero cap trips, extra cells reported), and to the Problem's evidence.
- **The SH floor is undefined in this brief.** The SH-reach row should say 256 MiB (`done/sh-streaming--reveal-gate-and-warm-horizon`).

**Mis-specified rows, or rows that gate the wrong quantity**

- **"The step cap is below its value before this change."** This asserts that a constant moved, which proves nothing. Gate the headroom instead: the lowered cap is at least 2× the worst rect pose across every swept map. That worst pose is currently occlusion-test at 1,987 steps [meas], so 5,000 gives 2.5×.
- **"The portal-walk epoch differs."** Same issue. It is fine as a review checklist item; better, assert that the id-51 cache key changes.
- **The grep gate over "lower bound" / "any (runtime )?FOV".** This is a prose lint. It would false-positive on correct restated text such as "no longer a lower bound". Replace it with the dry-run test asserting the restated report string.
- **"No cell expands more than 32 times" and "k × summed degree".** These are regression guards on fixtures, not proof of the §2 bound. With Q = 256 the lattice allows about 4·(2Q+1) ≈ 2,050 growths per cell. The brief's Problem says cost "is bounded per visible cell"; it should say the theoretical constant is large and the cap is the backstop. The measured worst is 17 expansions per cell (hallway) and 16 (env-volumes) [meas], so 32 is a sensible guard.
- **The residency gate's baseline is biased.**
  - On a step-limit frame, lightmap demand is `PathDemand::CameraSet`: drawn cells are not demanded in play (`cell_demand.rs:93-113`) [src].
  - So the pre-change build's trip frames add no reads and log visible misses instead, which the gate doesn't count. The rect build demands those frames' drawn blocks and can grow.
  - A "zero baseline requires zero" gate can therefore fail on trip-heavy runs: ±45° on the hallway, or any lake fixture. That failure would reflect the old build refusing demand, not over-inclusion.
  - Fix: count the pre-change build's drawn-but-unread blocks on fallback frames beside its pool deferrals, or exclude poses where the pre-change walk tripped. Report the trip count per run either way.

**Redundant rows.** "Every existing traversal test that expects a cell visible still passes" and "the camera cell is always visible" overlap with the set-shape row. Keep all three; they cost little.

**Achievability.** `cargo test -p postretro-visibility` does not compile at HEAD [meas]. `crates/level-loader/src/prl.rs:64` imports `TeleportersSection` without `#[cfg(feature = "load-prl")]` (from 86bec6c83). The visibility crate uses the loader with `default-features = false`. With `--features postretro-level-loader/load-prl`, 54 tests pass. Either fix it in the split commit or note the flag; executors will hit it first.

**Manual block.** The rows are individually sound, but they need new instruments:
- `walk_measurement` modes for projection, pitch and synchronous reads, plus cause buckets;
- a Settling log of step-limit frames and settle-set peaks;
- a pre-change build that carries all of these;
- rebakes of the 3.3 GB hallway.

The residency row alone is 3 projections × 3 pitches × 2 walks × 2 builds, plus movement-feel. That is resumable-brief scope.

## 5. Is it ready to promote? The smallest set of changes

1. **Decisions:** add the seeded entry as a Decision, with the two rows above. Mark the SH dependency as landed (`done/`, c63ddd09a) and drop O20.
2. **Problem and sweep row:** cite the lake (3.9% of walks; every frame at spawn, 3.7 ms of 10.4 ms work, dev profile, Metal 5300M) and occlusion-test (3.0%). Add both maps to the manual sweep.
3. **Residency gate:** correct the baseline as described under §4.
4. **Rows:** replace the two constant-moved rows with a headroom row and a cache-key row. Replace the grep gate with a string assertion. State the 256 MiB SH floor.
5. **Oracle:** in Path, intersect the bypass's narrowed frustum with the parent frustum.
6. **Execution mode:** either relabel the brief resumable, or move the instrument commits (O19) into a precursor brief so this one stays compact.
7. **Housekeeping:** drop the "SH does not stream is stale" line, and note the `postretro-visibility` test-build break.

## 6. Where would reviewers plausibly disagree?

| Tension | For strict / wide | For loose / narrow | Where I land |
|---|---|---|---|
| **Bake-gap gate, zero-relative** | A deferral is a visible miss of a block that may truly be visible, so zero should mean zero. | The synchronous mode overstates deferrals (O18), and the baseline undercounts (§4). | Keep the gate relative and strict, fix the baseline, and keep "fail → owner decision". Strictness is fine once the baseline is honest. |
| **Superset over-inclusion cost** | It is small on the hallway and campaign maps (p95 +2–4). | It reaches p95 +21 and max +182 on occlusion-test, and fixed-capacity consumers pay for it. | Accept rect. It is a large improvement on every trip pose, where today's consumers get the frustum-all set. Make occlusion-test part of the reported sweep so the octagon or bitmask swap has a measured trigger. |
| **Rect vs restart-hybrid** | Hybrid is exact on ordinary poses. | It costs about 2× the µs, and the bake would sample a walk whose meaning depends on the pose. | Rect. |
| **Scope: compact vs resumable** | The walk change itself is compact. | Proving the bake gap needs new instruments and a dozens-of-runs manual matrix. | Resumable, or a precursor instrument brief. |
| **Seeded entry here or in planar** | Planar owns the mirror. | §2 already places the mirror on this walk, the core is one function, and leaving it out re-blocks planar. | Here, as a Decision with rows. |
| **Bypass tightening in scope** | It is the largest source of over-inclusion (hallway bypass poses: p95 +16 vs +3 without). | It is a behaviour change on top of a rewrite. | In scope. It is what makes the superset tight, and the oracle needs it too. |

## Couldn't verify

- Rows that depend on instruments that don't exist yet: residency, settle and SH reach.
- Hallway live frame time.
- Release-profile live numbers.
- The Windows/GTX numbers quoted from planar's research.
- The sources in the research's algorithm survey: I didn't re-fetch Descent's `RENDER.C` or Source's `r_areaportal.cpp`. The characterisations match my prior knowledge of both, but I haven't re-checked them against the source text.
- Whether every rect miss on campaign-test (24 poses), the lake (4), occlusion-test (2) and env-volumes (8) falls into the brief's exception classes. I replayed one campaign-test pose and one lake pose, and both did.
